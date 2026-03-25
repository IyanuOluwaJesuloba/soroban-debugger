use crate::runtime::executor::ContractExecutor;
use crate::{DebuggerError, Result};
use serde::Serialize;
use std::cmp;
use std::collections::HashSet;
use std::fmt::Write;
use wasmparser::{Parser, Payload};

#[derive(Debug, Clone, Serialize)]
pub struct PathResult {
    pub inputs: String, // json array of args
    pub return_value: Option<String>,
    pub panic: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolicReport {
    pub function: String,
    pub paths_explored: usize,
    pub panics_found: usize,
    pub paths: Vec<PathResult>,
}

#[derive(Default)]
pub struct SymbolicAnalyzer;

impl SymbolicAnalyzer {
    pub fn new() -> Self {
        Self
    }

    fn record_outcome(
        report: &mut SymbolicReport,
        seen_inputs: &mut HashSet<String>,
        inputs: &str,
        outcome: std::result::Result<String, String>,
    ) {
        // Keep distinct paths even when outputs/errors are identical.
        // Only dedupe when the exact same input set is re-encountered.
        if !seen_inputs.insert(inputs.to_string()) {
            return;
        }

        match outcome {
            Ok(val) => report.paths.push(PathResult {
                inputs: inputs.to_string(),
                return_value: Some(val),
                panic: None,
            }),
            Err(err_str) => {
                report.panics_found += 1;
                report.paths.push(PathResult {
                    inputs: inputs.to_string(),
                    return_value: None,
                    panic: Some(err_str),
                });
            }
        }
    }

    pub fn analyze(&self, wasm: &[u8], function: &str) -> Result<SymbolicReport> {
        let arg_count = self.get_arg_count(wasm, function).unwrap_or(0);
        let combinations = self.generate_input_combinations(arg_count);

        let mut report = SymbolicReport {
            function: function.to_string(),
            paths_explored: 0,
            panics_found: 0,
            paths: Vec::new(),
        };

        let mut seen_inputs = HashSet::new();

        for args_json in combinations.iter().take(100) {
            let executor_res = std::panic::catch_unwind(|| {
                if let Ok(mut executor) = ContractExecutor::new(wasm.to_vec()) {
                    executor.execute(function, Some(args_json))
                } else {
                    Err(crate::DebuggerError::ExecutionError("Init fail".into()).into())
                }
            });

            match executor_res {
                Ok(Ok(val)) => {
                    Self::record_outcome(&mut report, &mut seen_inputs, args_json, Ok(val));
                }
                Ok(Err(err)) => {
                    Self::record_outcome(
                        &mut report,
                        &mut seen_inputs,
                        args_json,
                        Err(err.to_string()),
                    );
                }
                Err(_) => {
                    Self::record_outcome(
                        &mut report,
                        &mut seen_inputs,
                        args_json,
                        Err("Host Panic".to_string()),
                    );
                }
            }
            report.paths_explored += 1;
        }

        Ok(report)
    }

    fn get_arg_count(&self, wasm: &[u8], target: &str) -> Result<usize> {
        let parser = Parser::new(0);
        let mut type_definitions = Vec::new();
        let mut function_types = Vec::new();
        let mut exports = Vec::new();
        let mut imported_func_count: u32 = 0;

        for payload in parser.parse_all(wasm) {
            match payload
                .map_err(|e| DebuggerError::WasmLoadError(format!("Failed to parse WASM: {}", e)))?
            {
                Payload::ImportSection(reader) => {
                    for import in reader {
                        let import = import.map_err(|e| {
                            DebuggerError::WasmLoadError(format!(
                                "Failed to read import section: {}",
                                e
                            ))
                        })?;
                        if matches!(import.ty, wasmparser::TypeRef::Func(_)) {
                            imported_func_count += 1;
                        }
                    }
                }
                Payload::TypeSection(reader) => {
                    for rec_group in reader {
                        let rec_group = rec_group.map_err(|e| {
                            DebuggerError::WasmLoadError(format!(
                                "Failed to read type section: {}",
                                e
                            ))
                        })?;
                        for ty in rec_group.types() {
                            if let wasmparser::CompositeType::Func(func_type) = &ty.composite_type {
                                type_definitions.push(func_type.clone());
                            }
                        }
                    }
                }
                Payload::FunctionSection(reader) => {
                    for type_idx in reader {
                        function_types.push(type_idx.map_err(|e| {
                            DebuggerError::WasmLoadError(format!(
                                "Failed to read function section: {}",
                                e
                            ))
                        })?);
                    }
                }

                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.map_err(|e| {
                            DebuggerError::WasmLoadError(format!(
                                "Failed to read export section: {}",
                                e
                            ))
                        })?;
                        if let wasmparser::ExternalKind::Func = export.kind {
                            exports.push((export.name.to_string(), export.index));
                        }
                    }
                }
                _ => {}
            }
        }

        for (name, func_idx) in exports {
            if name == target {
                if func_idx < imported_func_count {
                    continue;
                }
                let local_idx = (func_idx - imported_func_count) as usize;
                if let Some(&type_idx) = function_types.get(local_idx) {
                    if let Some(func_type) = type_definitions.get(type_idx as usize) {
                        return Ok(func_type.params().len());
                    }
                }
            }
        }

        Err(
            DebuggerError::InvalidFunction(format!("Function '{}' not found in exports", target))
                .into(),
        )
    }

    fn generate_input_combinations(&self, arg_count: usize) -> Vec<String> {
        // Values representing symbolic extremes
        let values = vec!["0", "1", "-1", "42", "2147483647", "-2147483648"];
        const MAX_CASES: usize = 256;

        let mut combinations = Vec::new();
        if arg_count == 0 {
            combinations.push("[]".to_string());
            return combinations;
        }

        if arg_count == 1 {
            for v in &values {
                combinations.push(format!("[{}]", v));
            }
            return combinations;
        }

        if arg_count == 2 {
            for v1 in &values {
                for v2 in &values {
                    combinations.push(format!("[{}, {}]", v1, v2));
                }
            }
            return combinations;
        }

        // Generic cartesian product for 3+ args with a capped exploration budget.
        // Keep breadth while avoiding exponential blowups.
        let narrowed = &values[..cmp::min(values.len(), 4)];
        let mut current = vec![0usize; arg_count];
        loop {
            let args = current
                .iter()
                .map(|&idx| narrowed[idx])
                .collect::<Vec<_>>()
                .join(", ");
            combinations.push(format!("[{}]", args));

            if combinations.len() >= MAX_CASES {
                break;
            }

            let mut carry = true;
            for pos in (0..arg_count).rev() {
                if current[pos] + 1 < narrowed.len() {
                    current[pos] += 1;
                    for slot in current.iter_mut().skip(pos + 1) {
                        *slot = 0;
                    }
                    carry = false;
                    break;
                }
            }
            if carry {
                break;
            }
        }
        combinations
    }

    pub fn generate_scenario_toml(&self, report: &SymbolicReport) -> String {
        let mut toml = String::new();
        writeln!(toml, "# Generated Symbolic Execution Scenarios").unwrap();
        writeln!(toml, "function = {}", toml_basic_string(&report.function)).unwrap();
        writeln!(toml, "paths_explored = {}", report.paths_explored).unwrap();
        writeln!(toml, "panics_found = {}\n", report.panics_found).unwrap();

        for (i, path) in report.paths.iter().enumerate() {
            writeln!(toml, "[[scenario]]").unwrap();
            writeln!(toml, "id = {}", i).unwrap();
            writeln!(toml, "inputs = {}", toml_basic_string(&path.inputs)).unwrap();

            if let Some(ref val) = path.return_value {
                writeln!(toml, "expected_return = {}", toml_basic_string(val)).unwrap();
            }
            if let Some(ref panic) = path.panic {
                writeln!(toml, "panic = {}", toml_basic_string(panic)).unwrap();
            }
            writeln!(toml).unwrap();
        }

        toml
    }
}

fn toml_basic_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{}\"", escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_u32_leb(mut value: u32, out: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn push_name(name: &str, out: &mut Vec<u8>) {
        push_u32_leb(name.len() as u32, out);
        out.extend_from_slice(name.as_bytes());
    }

    fn append_section(module: &mut Vec<u8>, section_id: u8, section_data: &[u8]) {
        module.push(section_id);
        push_u32_leb(section_data.len() as u32, module);
        module.extend_from_slice(section_data);
    }

    fn wasm_with_import_and_exported_local() -> Vec<u8> {
        let mut module = Vec::new();
        module.extend_from_slice(&[0x00, 0x61, 0x73, 0x6d]);
        module.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);

        // Type section: type 0 = () -> (), type 1 = (i64, i64) -> ()
        let mut types = Vec::new();
        push_u32_leb(2, &mut types);
        types.push(0x60);
        push_u32_leb(0, &mut types);
        push_u32_leb(0, &mut types);
        types.push(0x60);
        push_u32_leb(2, &mut types);
        types.push(0x7e);
        types.push(0x7e);
        push_u32_leb(0, &mut types);
        append_section(&mut module, 1, &types);

        // Import section: one imported function using type 0
        let mut imports = Vec::new();
        push_u32_leb(1, &mut imports);
        push_name("env", &mut imports);
        push_name("imported", &mut imports);
        imports.push(0x00);
        push_u32_leb(0, &mut imports);
        append_section(&mut module, 2, &imports);

        // Function section: one local function using type 1
        let mut functions = Vec::new();
        push_u32_leb(1, &mut functions);
        push_u32_leb(1, &mut functions);
        append_section(&mut module, 3, &functions);

        // Export section: export local function at global index 1
        let mut exports = Vec::new();
        push_u32_leb(1, &mut exports);
        push_name("entry", &mut exports);
        exports.push(0x00);
        push_u32_leb(1, &mut exports);
        append_section(&mut module, 7, &exports);

        // Code section: one empty function body
        let mut code = Vec::new();
        push_u32_leb(1, &mut code);
        let body = vec![0x00, 0x0b];
        push_u32_leb(body.len() as u32, &mut code);
        code.extend_from_slice(&body);
        append_section(&mut module, 10, &code);

        module
    }

    #[test]
    fn distinct_inputs_with_same_output_are_not_deduped() {
        let mut report = SymbolicReport {
            function: "f".to_string(),
            paths_explored: 0,
            panics_found: 0,
            paths: Vec::new(),
        };
        let mut seen_inputs = HashSet::new();

        SymbolicAnalyzer::record_outcome(&mut report, &mut seen_inputs, "[0]", Ok("1".into()));
        SymbolicAnalyzer::record_outcome(&mut report, &mut seen_inputs, "[1]", Ok("1".into()));

        assert_eq!(report.paths.len(), 2);
        assert_eq!(report.panics_found, 0);
        assert_eq!(report.paths[0].return_value.as_deref(), Some("1"));
        assert_eq!(report.paths[1].return_value.as_deref(), Some("1"));
    }

    #[test]
    fn identical_inputs_are_deduped() {
        let mut report = SymbolicReport {
            function: "f".to_string(),
            paths_explored: 0,
            panics_found: 0,
            paths: Vec::new(),
        };
        let mut seen_inputs = HashSet::new();

        SymbolicAnalyzer::record_outcome(&mut report, &mut seen_inputs, "[0]", Ok("1".into()));
        SymbolicAnalyzer::record_outcome(&mut report, &mut seen_inputs, "[0]", Ok("1".into()));

        assert_eq!(report.paths.len(), 1);
    }

    #[test]
    fn get_arg_count_accounts_for_imported_function_offset() {
        let wasm = wasm_with_import_and_exported_local();
        let analyzer = SymbolicAnalyzer::new();

        let arg_count = analyzer
            .get_arg_count(&wasm, "entry")
            .expect("entry export should resolve");

        assert_eq!(arg_count, 2);
    }
}
