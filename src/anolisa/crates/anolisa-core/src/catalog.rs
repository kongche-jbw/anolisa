//! Catalog: layered loader for capability and component manifests.
//!
//! Three layers are supported and applied in order of increasing precedence:
//!
//! 1. `bundled` — the manifests shipped with the source tree (always present).
//! 2. `system` — `/etc/anolisa/manifests` (optional, ops overrides).
//! 3. `user`   — `~/.config/anolisa/manifests` (optional, per-user overrides).
//!
//! Within each layer the loader walks
//! `capabilities/*.toml`, `runtime/*.toml`, `osbase/*.toml` and keys entries by
//! manifest name. A later layer with the same key replaces the earlier entry.
//!
//! `Catalog::load` is intentionally tolerant: missing layer directories are
//! ignored and individual malformed manifests surface as `CatalogError`s
//! rather than panicking.

use crate::manifest::{CapabilityManifest, ComponentManifest, ManifestError, manifest_paths};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CatalogLayers {
    pub system: Option<PathBuf>,
    pub user: Option<PathBuf>,
    pub bundled: PathBuf,
}

impl CatalogLayers {
    /// Helper for the common case of a bundled-only catalog (used by tests
    /// and by the CLI when no overrides are configured).
    pub fn bundled_only(bundled: PathBuf) -> Self {
        Self {
            system: None,
            user: None,
            bundled,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub capabilities: BTreeMap<String, CapabilityManifest>,
    pub components: BTreeMap<String, ComponentManifest>,
    pub layers: CatalogLayers,
}

impl Catalog {
    /// Load the catalog from disk, walking each configured layer in
    /// precedence order. A missing optional layer is silently skipped.
    pub fn load(layers: CatalogLayers) -> Result<Self, CatalogError> {
        let mut capabilities: BTreeMap<String, CapabilityManifest> = BTreeMap::new();
        let mut components: BTreeMap<String, ComponentManifest> = BTreeMap::new();

        let layered: [Option<&Path>; 3] = [
            Some(layers.bundled.as_path()),
            layers.system.as_deref(),
            layers.user.as_deref(),
        ];

        for layer_root in layered.into_iter().flatten() {
            load_layer(layer_root, &mut capabilities, &mut components)?;
        }

        Ok(Self {
            capabilities,
            components,
            layers,
        })
    }

    pub fn capability(&self, name: &str) -> Option<&CapabilityManifest> {
        self.capabilities.get(name)
    }

    pub fn component(&self, name: &str) -> Option<&ComponentManifest> {
        self.components.get(name)
    }

    pub fn list_capabilities(&self) -> Vec<&CapabilityManifest> {
        self.capabilities.values().collect()
    }

    pub fn list_components(&self) -> Vec<&ComponentManifest> {
        self.components.values().collect()
    }
}

fn load_layer(
    root: &Path,
    capabilities: &mut BTreeMap<String, CapabilityManifest>,
    components: &mut BTreeMap<String, ComponentManifest>,
) -> Result<(), CatalogError> {
    if !root.exists() {
        return Ok(());
    }

    for path in manifest_paths(&root.join("capabilities")) {
        let m = CapabilityManifest::from_file(&path).map_err(CatalogError::from)?;
        capabilities.insert(m.capability.name.clone(), m);
    }

    for sub in ["runtime", "osbase"] {
        for path in manifest_paths(&root.join(sub)) {
            let m = ComponentManifest::from_file(&path).map_err(CatalogError::from)?;
            components.insert(m.component.name.clone(), m);
        }
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn bundled_root() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("..");
        p.push("..");
        p.push("manifests");
        p.canonicalize().expect("bundled manifests path resolves")
    }

    #[test]
    fn loads_bundled_catalog() {
        let catalog = Catalog::load(CatalogLayers::bundled_only(bundled_root()))
            .expect("bundled catalog loads");
        // Spot-check a few canonical names.
        assert!(catalog.capability("agent-observability").is_some());
        assert!(catalog.capability("token-optimization").is_some());
        assert!(catalog.component("agentsight").is_some());
        assert!(catalog.component("tokenless").is_some());
        // Layer scan should pick up all bundled fixtures.
        assert!(
            catalog.list_capabilities().len() >= 9,
            "expected at least 9 capabilities, got {}",
            catalog.list_capabilities().len()
        );
        assert!(
            catalog.list_components().len() >= 6,
            "expected at least 6 components, got {}",
            catalog.list_components().len()
        );
    }

    #[test]
    fn user_layer_overrides_bundled() {
        let tmp = tempdir();
        let cap_dir = tmp.path().join("capabilities");
        fs::create_dir_all(&cap_dir).expect("mkdir cap_dir");
        let override_toml = r#"
            [capability]
            name = "agent-observability"
            description = "USER LAYER OVERRIDE"

            [implementation]
            components = ["agentsight"]

            [requires_env]
            os = "linux"
        "#;
        fs::write(cap_dir.join("agent-observability.toml"), override_toml).expect("write override");

        let layers = CatalogLayers {
            system: None,
            user: Some(tmp.path().to_path_buf()),
            bundled: bundled_root(),
        };
        let catalog = Catalog::load(layers).expect("load with override");
        let m = catalog
            .capability("agent-observability")
            .expect("capability present");
        assert_eq!(m.capability.description, "USER LAYER OVERRIDE");
    }

    #[test]
    fn lookup_roundtrip() {
        let catalog = Catalog::load(CatalogLayers::bundled_only(bundled_root()))
            .expect("bundled catalog loads");

        let cap = catalog
            .capability("agent-observability")
            .expect("agent-observability present");
        assert_eq!(cap.capability.name, "agent-observability");

        let comp = catalog.component("agentsight").expect("agentsight present");
        assert_eq!(comp.component.name, "agentsight");
    }

    #[test]
    fn system_layer_then_user_layer_precedence() {
        let sys = tempdir();
        let usr = tempdir();
        fs::create_dir_all(sys.path().join("capabilities")).expect("mkdir sys cap");
        fs::create_dir_all(usr.path().join("capabilities")).expect("mkdir usr cap");
        fs::write(
            sys.path().join("capabilities/agent-memory.toml"),
            r#"
                [capability]
                name = "agent-memory"
                description = "SYSTEM"
                [implementation]
                components = ["agent-memory"]
                [requires_env]
                os = "linux"
            "#,
        )
        .expect("write sys");
        fs::write(
            usr.path().join("capabilities/agent-memory.toml"),
            r#"
                [capability]
                name = "agent-memory"
                description = "USER"
                [implementation]
                components = ["agent-memory"]
                [requires_env]
                os = "linux"
            "#,
        )
        .expect("write usr");

        let layers = CatalogLayers {
            system: Some(sys.path().to_path_buf()),
            user: Some(usr.path().to_path_buf()),
            bundled: bundled_root(),
        };
        let catalog = Catalog::load(layers).expect("load layered");
        let m = catalog
            .capability("agent-memory")
            .expect("agent-memory present");
        assert_eq!(m.capability.description, "USER");
    }

    // ----- Lightweight tempdir helper (no extra dependency). -----

    struct TempDir(PathBuf);

    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> TempDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let counter = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("anolisa-core-catalog-{pid}-{nanos}-{counter}"));
        fs::create_dir_all(&path).expect("create tempdir");
        TempDir(path)
    }

    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
}
