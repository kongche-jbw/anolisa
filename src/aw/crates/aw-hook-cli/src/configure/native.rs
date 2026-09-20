//! Selected installed-file provenance, independent of untrusted workspace content.

use aw_host_process::{FilePin, PinState};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

pub(super) fn read(path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("read admitted file {}: {error}", path.display()))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(format!(
            "admitted file is not regular or exceeds limit: {}",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > maximum {
        return Err("admitted file grew beyond its size limit".into());
    }
    Ok(bytes)
}

pub(super) fn digest(path: &Path) -> Result<String, String> {
    bounded_digest(path, 64 * 1024 * 1024)
}

pub(super) fn bounded_digest(path: &Path, maximum: u64) -> Result<String, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("read admitted executable {}: {error}", path.display()))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(format!(
            "admitted executable exceeds its file limit: {}",
            path.display()
        ));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65_536];
    let mut total = 0;
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > maximum {
            return Err("admitted executable grew beyond its size limit".into());
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn sec_core_pins(
    program: &Path,
    additional: &[PathBuf],
) -> Result<Vec<FilePin>, String> {
    let bytes = read(program, 64 * 1024 * 1024)?;
    let mut paths = Vec::new();
    if bytes.starts_with(b"#!") {
        let line = bytes
            .split(|byte| *byte == b'\n')
            .next()
            .ok_or("empty SecCore entrypoint")?;
        let interpreter = std::str::from_utf8(&line[2..])
            .map_err(|_| "SecCore interpreter must be UTF-8")?
            .trim();
        if !Path::new(interpreter).is_absolute() || interpreter.split_whitespace().count() != 1 {
            return Err("SecCore entrypoint requires an absolute Python interpreter without shell arguments".into());
        }
        let interpreter = PathBuf::from(interpreter);
        let venv = interpreter
            .parent()
            .and_then(Path::parent)
            .ok_or("SecCore entrypoint must belong to an installed Python 3.11 environment")?;
        let site = venv.join("lib/python3.11/site-packages");
        let package = site.join("agent_sec_cli");
        paths.push(interpreter.clone());
        paths.push(venv.join("pyvenv.cfg"));
        paths.push(site.join("agent_sec_cli-0.12.0.dist-info/METADATA"));
        // These pins cover the admitted scanner path and selected default rules,
        // not Python's complete import graph or every installed policy file.
        for relative in [
            "__init__.py",
            "cli.py",
            "code_scanner/__init__.py",
            "code_scanner/scanner.py",
            "code_scanner/models.py",
            "code_scanner/errors.py",
            "code_scanner/engine/__init__.py",
            "code_scanner/engine/regex_engine.py",
            "code_scanner/engine/code_extractor.py",
            "code_scanner/rules/__init__.py",
            "code_scanner/rules/rule_loader.py",
            "code_scanner/rules/bash/_shared.yaml",
            "code_scanner/rules/bash/shell-download-exec.yaml",
            "code_scanner/rules/bash/shell-recursive-delete.yaml",
            "code_scanner/rules/bash/shell-system-file-delete.yaml",
            "security_middleware/__init__.py",
            "security_middleware/router.py",
            "security_middleware/lifecycle.py",
            "security_middleware/context.py",
            "security_middleware/result.py",
            "security_middleware/backends/base.py",
            "security_middleware/backends/code_scan.py",
        ] {
            paths.push(package.join(relative));
        }
        paths.push(package.join(format!(
            "_native.cpython-311-{}-linux-gnu.so",
            std::env::consts::ARCH
        )));
    } else if !bytes.starts_with(b"\x7fELF") {
        return Err(
            "SecCore must be an installed native CLI or its Python console entrypoint".into(),
        );
    }
    for path in additional {
        if path == program || paths.contains(path) {
            return Err(format!(
                "duplicate SecCore selected-file pin: {}",
                path.display()
            ));
        }
        paths.push(path.clone());
    }
    if paths.len() > 32 {
        return Err("SecCore exceeds the 32 selected-file pin limit".into());
    }
    paths
        .into_iter()
        .map(|path| {
            let hash = digest(&path)?;
            Ok(FilePin {
                path,
                state: PinState::Sha256(hash),
            })
        })
        .collect()
}
