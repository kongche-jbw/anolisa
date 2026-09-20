//! Apply an explicitly enabled AW shim after the native Bash startup files.

use super::model::ShellHostConfig;
use std::{
    fs::OpenOptions,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

pub(super) fn rcfile(
    config: &ShellHostConfig,
    marker: Option<&Path>,
) -> io::Result<Option<PathBuf>> {
    let Some(shim) = &config.aw_shim_directory else {
        return Ok(None);
    };
    let marker =
        marker.ok_or_else(|| io::Error::other("AW requires the Bash startup integration"))?;
    let quote = |value: &Path| format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"));
    let path = config.work_dir.join("aw-startup.bash");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)?;
    writeln!(
        file,
        "source {}\nexport PATH={}:\"$PATH\"\nhash -r",
        quote(marker),
        quote(shim)
    )?;
    Ok(Some(path))
}
