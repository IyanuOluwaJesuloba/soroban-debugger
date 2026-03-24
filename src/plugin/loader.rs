use super::api::{
    InspectorPlugin, PluginConstructor, PluginError, PluginResult, PLUGIN_CONSTRUCTOR_SYMBOL,
};
use super::manifest::PluginManifest;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

/// A loaded plugin instance
pub struct LoadedPlugin {
    /// The plugin instance
    plugin: Box<dyn InspectorPlugin>,

    /// The dynamic library handle
    #[allow(dead_code)]
    library: libloading::Library,

    /// Path to the plugin library
    path: PathBuf,

    /// Plugin manifest
    manifest: PluginManifest,
}

impl LoadedPlugin {
    /// Get a reference to the plugin
    pub fn plugin(&self) -> &dyn InspectorPlugin {
        &*self.plugin
    }

    /// Get a mutable reference to the plugin
    pub fn plugin_mut(&mut self) -> &mut dyn InspectorPlugin {
        &mut *self.plugin
    }

    /// Get the plugin manifest
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Get the plugin path
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Plugin loader that handles dynamic loading of plugin libraries
pub struct PluginLoader {
    /// Base directory for plugins
    plugin_dir: PathBuf,
}

impl PluginLoader {
    /// Create a new plugin loader
    pub fn new(plugin_dir: PathBuf) -> Self {
        Self { plugin_dir }
    }

    /// Get the default plugin directory (~/.soroban-debug/plugins/)
    pub fn default_plugin_dir() -> PluginResult<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| {
            PluginError::InitializationFailed("Could not determine home directory".to_string())
        })?;

        Ok(home.join(".soroban-debug").join("plugins"))
    }

    /// Load a plugin from a manifest file
    pub fn load_from_manifest(&self, manifest_path: &Path) -> PluginResult<LoadedPlugin> {
        info!("Loading plugin from manifest: {:?}", manifest_path);

        // Load and validate manifest
        let manifest = PluginManifest::from_file(&manifest_path.to_path_buf())
            .map_err(|e| PluginError::Invalid(format!("Failed to load manifest: {}", e)))?;

        manifest
            .validate()
            .map_err(|e| PluginError::Invalid(format!("Invalid manifest: {}", e)))?;

        // Resolve library path relative to manifest
        let manifest_dir = manifest_path
            .parent()
            .ok_or_else(|| PluginError::Invalid("Invalid manifest path".to_string()))?;

        let mut library_path = manifest_dir.join(&manifest.library);
        if !library_path.exists() {
            if let Some(fallback) = resolve_platform_library_path(manifest_dir, &manifest.library) {
                library_path = fallback;
            }
        }

        if !library_path.exists() {
            return Err(PluginError::NotFound(format!(
                "Plugin library not found: {:?}",
                library_path
            )));
        }

        // Load the dynamic library
        self.load_library(&library_path, manifest)
    }

    /// Load a plugin directly from a library path
    pub fn load_library(
        &self,
        library_path: &Path,
        manifest: PluginManifest,
    ) -> PluginResult<LoadedPlugin> {
        info!("Loading plugin library: {:?}", library_path);

        unsafe {
            // Load the library
            let library = libloading::Library::new(library_path).map_err(|e| {
                PluginError::InitializationFailed(format!("Failed to load library: {}", e))
            })?;

            // Get the constructor symbol
            let constructor: libloading::Symbol<PluginConstructor> = library
                .get(PLUGIN_CONSTRUCTOR_SYMBOL.as_bytes())
                .map_err(|e| {
                    PluginError::Invalid(format!(
                        "Plugin does not export '{}': {}",
                        PLUGIN_CONSTRUCTOR_SYMBOL, e
                    ))
                })?;

            // Create the plugin instance
            let plugin_ptr = constructor();
            if plugin_ptr.is_null() {
                return Err(PluginError::InitializationFailed(
                    "Plugin constructor returned null".to_string(),
                ));
            }

            let mut plugin = Box::from_raw(plugin_ptr);

            // Verify manifest matches
            let plugin_manifest = plugin.metadata();
            if plugin_manifest.name != manifest.name {
                warn!(
                    "Plugin manifest name mismatch: expected '{}', got '{}'",
                    manifest.name, plugin_manifest.name
                );
            }

            // Initialize the plugin
            plugin.initialize().map_err(|e| {
                PluginError::InitializationFailed(format!("Plugin initialization failed: {}", e))
            })?;

            info!(
                "Successfully loaded plugin: {} v{}",
                manifest.name, manifest.version
            );

            Ok(LoadedPlugin {
                plugin,
                library,
                path: library_path.to_path_buf(),
                manifest: manifest.clone(),
            })
        }
    }

    /// Discover all plugins in the plugin directory.
    ///
    /// Results are sorted by path so the discovery order is deterministic
    /// across platforms and file-system implementations.  The registry's
    /// topological sort handles dependency ordering; this sort ensures that
    /// unrelated plugins always appear in the same sequence, making behaviour
    /// reproducible and tests stable.
    pub fn discover_plugins(&self) -> Vec<PathBuf> {
        let mut manifests = Vec::new();

        if !self.plugin_dir.exists() {
            info!("Plugin directory does not exist: {:?}", self.plugin_dir);
            return manifests;
        }

        // Look for plugin.toml files in subdirectories
        if let Ok(entries) = std::fs::read_dir(&self.plugin_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let manifest_path = path.join("plugin.toml");
                    if manifest_path.exists() {
                        manifests.push(manifest_path);
                    }
                }
            }
        }

        // Sort for deterministic, platform-independent discovery order.
        // Dependency ordering is handled by the registry's topological sort.
        manifests.sort();

        info!("Discovered {} plugin manifests", manifests.len());
        manifests
    }

    /// Load all discovered plugins
    pub fn load_all(&self) -> Vec<PluginResult<LoadedPlugin>> {
        let manifests = self.discover_plugins();

        manifests
            .iter()
            .map(|manifest_path| self.load_from_manifest(manifest_path))
            .collect()
    }
}

fn resolve_platform_library_path(manifest_dir: &Path, library: &str) -> Option<PathBuf> {
    let wanted_ext = match std::env::consts::OS {
        "windows" => "dll",
        "macos" => "dylib",
        _ => "so",
    };

    let base = Path::new(library);
    let stem = base.file_stem()?.to_string_lossy();
    let file_name = base.file_name()?.to_string_lossy();

    let mut candidates = Vec::new();

    // 1) Same stem, correct extension.
    candidates.push(format!("{stem}.{wanted_ext}"));

    // 2) Try toggling the `lib` prefix for cross-platform portability.
    if wanted_ext == "dll" && file_name.starts_with("lib") {
        candidates.push(format!("{}.dll", stem.trim_start_matches("lib")));
    } else if wanted_ext != "dll" && !file_name.starts_with("lib") {
        candidates.push(format!("lib{stem}.{wanted_ext}"));
    }

    for candidate in candidates {
        let p = manifest_dir.join(candidate);
        if p.exists() {
            return Some(p);
        }
    }

    None
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        info!("Unloading plugin: {}", self.manifest.name);

        if let Err(e) = self.plugin.shutdown() {
            error!("Error shutting down plugin {}: {}", self.manifest.name, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_plugin_dir() {
        let dir = PluginLoader::default_plugin_dir();
        assert!(dir.is_ok());

        let path = dir.unwrap();
        assert!(path.ends_with(".soroban-debug/plugins"));
    }

    #[test]
    fn test_loader_creation() {
        let temp_dir = std::env::temp_dir();
        let loader = PluginLoader::new(temp_dir.clone());
        assert_eq!(loader.plugin_dir, temp_dir);
    }

    /// `discover_plugins` must return paths in sorted order so that repeated
    /// calls on the same directory yield the same sequence regardless of the
    /// order the OS returns directory entries.
    #[test]
    fn discover_plugins_returns_sorted_paths() {
        use std::fs;

        let base = std::env::temp_dir().join("soroban-loader-sort-test");
        let _ = fs::remove_dir_all(&base);

        // Create three plugin sub-directories in reverse alphabetical order so
        // a naive read_dir would likely return them unsorted.
        //............
        for name in &["plugin-c", "plugin-a", "plugin-b"] {
            let dir = base.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("plugin.toml"), "").unwrap();
        }

        let loader = PluginLoader::new(base.clone());
        let paths = loader.discover_plugins();

        let names: Vec<&str> = paths
            .iter()
            .filter_map(|p| p.parent()?.file_name()?.to_str())
            .collect();

        assert_eq!(names, vec!["plugin-a", "plugin-b", "plugin-c"]);

        let _ = fs::remove_dir_all(&base);
    }
}