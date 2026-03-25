use crate::{DebuggerError, Result};
use gimli::{Dwarf, EndianSlice, RunTimeEndian};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use wasmparser::{Parser, Payload};

/// Represents a source code location
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: u32,
    pub column: Option<u32>,
}

/// Manages mapping from WASM offsets to source code locations
pub struct SourceMap {
    /// Mapping from offset to source location (sorted by offset)
    offsets: BTreeMap<usize, SourceLocation>,
    /// Cache of source file contents
    source_cache: HashMap<PathBuf, String>,
    /// Code section payload range (when known), used to normalize DWARF addresses.
    code_section_range: Option<std::ops::Range<usize>>,
}

impl Default for SourceMap {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceMap {
    /// Create a new empty source map
    pub fn new() -> Self {
        Self {
            offsets: BTreeMap::new(),
            source_cache: HashMap::new(),
            code_section_range: None,
        }
    }

    /// Load debug info from WASM bytes and build the mapping
    pub fn load(&mut self, wasm_bytes: &[u8]) -> Result<()> {
        let obj = object::File::parse(wasm_bytes)
            .map_err(|e| DebuggerError::WasmLoadError(format!("Failed to parse WASM object file: {}", e)))?;

        let load_section =
            |id: gimli::SectionId| -> std::result::Result<EndianSlice<RunTimeEndian>, gimli::Error> {
                let data = obj
                    .section_by_name(id.name())
                    .or_else(|| obj.section_by_name(&format!(".{}", id.name())))
                    .and_then(|s| s.data().ok())
                    .unwrap_or(&[]);
                Ok(EndianSlice::new(data, RunTimeEndian::Little))
            };

        let dwarf = Dwarf::load(&load_section)
            .map_err(|e| DebuggerError::WasmLoadError(format!("Failed to load DWARF sections: {}", e)))?;
        self.offsets.clear();
        self.code_section_range = crate::utils::wasm::code_section_range(wasm_bytes)?;

        let mut custom_sections: HashMap<String, &[u8]> = HashMap::new();
        for payload in Parser::new(0).parse_all(wasm_bytes) {
            let payload = payload.map_err(|e| {
                DebuggerError::WasmLoadError(format!("Failed to parse WASM: {}", e))
            })?;
            if let Payload::CustomSection(reader) = payload {
                custom_sections.insert(reader.name().to_string(), reader.data());
            }
        }

        let load_section = |id: gimli::SectionId| -> std::result::Result<EndianSlice<RunTimeEndian>, gimli::Error> {
            let name = id.name();
            let data = custom_sections
                .get(name)
                .or_else(|| custom_sections.get(&format!(".{}", name)))
                .or_else(|| custom_sections.get(name.trim_start_matches('.')))
                .copied()
                .unwrap_or(&[]);

            Ok(EndianSlice::new(data, RunTimeEndian::Little))
        };

        let dwarf = Dwarf::load(&load_section).map_err(|e| {
            DebuggerError::WasmLoadError(format!("Failed to load DWARF sections: {}", e))
        })?;

        let mut units = dwarf.units();
        while let Some(header) = units.next().map_err(|e| {
            DebuggerError::WasmLoadError(format!("Failed to read DWARF unit: {}", e))
        })? {
            let unit = dwarf.unit(header).map_err(|e| {
                DebuggerError::WasmLoadError(format!("Failed to load DWARF unit: {}", e))
            })?;
            if let Some(program) = unit.line_program.clone() {
                let mut rows = program.rows();
                while let Some((header, row)) = rows.next_row().map_err(|e| {
                    DebuggerError::WasmLoadError(format!("Failed to read DWARF line row: {}", e))
                })? {
                    if let Some(file_path) =
                        self.get_file_path(&dwarf, &unit, header, row.file_index())
                    {
                        // In WASM, DWARF addresses are usually offsets into the code section
                        let offset = row.address() as usize;
                        let line = row.line().map(|l: std::num::NonZeroU64| l.get() as u32).unwrap_or(0);
                        let column = match row.column() {
                            gimli::ColumnType::LeftEdge => None,
                            gimli::ColumnType::Column(c) => Some(c.get() as u32),
                        let offset =
                            self.normalize_wasm_offset(row.address() as usize, wasm_bytes.len());
                        let line = row.line().map(|l| l.get() as u32).unwrap_or(0);
                        let column = match row.column() {
                            gimli::ColumnType::LeftEdge => None,
                            gimli::ColumnType::Column(column) => Some(column.get() as u32),
                        };

                        self.offsets.insert(
                            offset,
                            SourceLocation {
                                file: file_path,
                                line,
                                column,
                            },
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Returns `true` if no mappings were loaded.
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// Number of mapped offsets.
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// Iterate over mappings as `(offset, location)` pairs.
    pub fn mappings(&self) -> impl Iterator<Item = (usize, &SourceLocation)> {
        self.offsets.iter().map(|(o, l)| (*o, l))
    }

    fn normalize_wasm_offset(&self, dwarf_address: usize, wasm_len: usize) -> usize {
        let Some(code_range) = &self.code_section_range else {
            return dwarf_address;
        };

        // Common case: DWARF line-program addresses are offsets into the code-section payload.
        let code_start = code_range.start;
        let code_len = code_range.end.saturating_sub(code_range.start);

        // If the address already looks like a module/file offset, keep it.
        if dwarf_address >= code_start && dwarf_address < wasm_len {
            return dwarf_address;
        }

        // Otherwise, treat addresses within the code-section payload length as relative.
        if dwarf_address < code_len {
            return code_start.saturating_add(dwarf_address);
        }

        dwarf_address
    }

    fn get_file_path(
        &self,
        dwarf: &Dwarf<EndianSlice<RunTimeEndian>>,
        unit: &gimli::Unit<EndianSlice<RunTimeEndian>>,
        header: &gimli::LineProgramHeader<EndianSlice<RunTimeEndian>>,
        file_index: u64,
    ) -> Option<PathBuf> {
        let file = header.file(file_index)?;
        let mut path = PathBuf::new();

        if let Some(directory) = file.directory(header) {
            let dir_attr = dwarf.attr_string(unit, directory).ok()?;
            path.push(dir_attr.to_string_lossy().as_ref());
        }

        let file_name_attr = dwarf.attr_string(unit, file.path_name()).ok()?;
        path.push(file_name_attr.to_string_lossy().as_ref());

        Some(path)
    }

    /// Lookup source location for a given WASM offset
    pub fn lookup(&self, offset: usize) -> Option<SourceLocation> {
        // Find the last entry with Key <= offset using BTreeMap
        self.offsets
            .range(..=offset)
            .next_back()
            .map(|(_, loc)| loc.clone())
    }

    /// (Internal/Test) Manually add a mapping
    pub fn add_mapping(&mut self, offset: usize, loc: SourceLocation) {
        self.offsets.insert(offset, loc);
    }

    /// Get source code line for a given location
    pub fn get_source_line(&mut self, location: &SourceLocation) -> Option<String> {
        let content = self.get_source_content(&location.file)?;
        content
            .lines()
            .nth(location.line.saturating_sub(1) as usize)
            .map(|s| s.to_string())
    }

    /// Get full source content, with caching
    pub fn get_source_content(&mut self, path: &Path) -> Option<&str> {
        if !self.source_cache.contains_key(path) {
            if let Ok(content) = fs::read_to_string(path) {
                self.source_cache.insert(path.to_path_buf(), content);
            } else {
                return None;
            }
        }
        self.source_cache.get(path).map(|s| s.as_str())
    }

    /// Clear the source cache
    pub fn clear_cache(&mut self) {
        self.source_cache.clear();
    }
}

impl Default for SourceMap {
    fn default() -> Self {
        Self::new()
    }
}
