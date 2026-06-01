//! Shared helpers for tier1 / tier2 command handlers.
//!
//! Read-only access to the three skeleton-stable objects:
//! [`FsLayout`], [`InstalledState`], and [`Catalog`]. Keep this module thin —
//! handlers compose these calls; we do not introduce a service layer here.

use std::path::{Path, PathBuf};

use anolisa_core::{Catalog, CatalogLayers, InstalledState};
use anolisa_platform::fs_layout::FsLayout;

use crate::context::{CliContext, InstallMode};
use crate::response::CliError;

/// Build the layout for the active install mode, honoring `--prefix`
/// (system-mode) and resolving `$HOME` via `EnvService::detect` (user-mode).
pub fn resolve_layout(ctx: &CliContext) -> FsLayout {
    match ctx.install_mode {
        InstallMode::System => FsLayout::system(ctx.prefix.clone()),
        InstallMode::User => {
            let home = anolisa_env::EnvService::detect().home;
            FsLayout::user(home)
        }
    }
}

/// Load `InstalledState` from the layout's `state_dir/installed.toml`.
/// A missing file yields `Default` — fresh installs are not an error.
pub fn load_installed_state(ctx: &CliContext, command: &str) -> Result<InstalledState, CliError> {
    let layout = resolve_layout(ctx);
    let path = layout.state_dir.join("installed.toml");
    InstalledState::load(&path).map_err(|err| CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!(
            "failed to load installed state at {}: {err}",
            path.display()
        ),
    })
}

/// Load the bundled catalog. Prefers `FsLayout::manifests_overlay` if the
/// directory exists; otherwise falls back to the in-tree manifests root
/// (`CARGO_MANIFEST_DIR/../../manifests`) so dev-tree runs work without
/// a real install layout.
pub fn load_bundled_catalog(ctx: &CliContext, command: &str) -> Result<Catalog, CliError> {
    let bundled = bundled_manifests_root(ctx);
    let layers = CatalogLayers {
        bundled,
        system: None,
        user: None,
    };
    Catalog::load(layers).map_err(|err| CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!("failed to load bundled catalog: {err}"),
    })
}

fn bundled_manifests_root(ctx: &CliContext) -> PathBuf {
    let overlay = resolve_layout(ctx).manifests_overlay;
    if overlay.is_dir() {
        return overlay;
    }
    dev_tree_manifests().unwrap_or(overlay)
}

fn dev_tree_manifests() -> Option<PathBuf> {
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("manifests");
    candidate.is_dir().then(|| candidate)
}
