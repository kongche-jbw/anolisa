//! Bounded native JSON input preserves numbers and keys outside AW metadata.

use crate::{Error, Settings, MAX_INPUT_BYTES};
use aw_contracts::canonical;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::{
    fmt,
    fs::OpenOptions,
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

/// Reads a private, regular launcher settings file using the AW metadata parser.
///
/// # Errors
/// Rejects non-absolute, oversized, symlinked or non-private files and invalid settings.
pub fn read_settings(path: &Path) -> Result<Settings, Error> {
    if !path.is_absolute() || !cfg!(target_os = "linux") {
        return Err(Error::Input);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| Error::Input)?;
    let metadata = file.metadata().map_err(|_| Error::Input)?;
    if !metadata.is_file() {
        return Err(Error::Input);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let caller = std::fs::metadata("/proc/self")
            .map_err(|_| Error::Input)?
            .uid();
        if metadata.uid() != caller || metadata.mode() & 0o077 != 0 {
            return Err(Error::Input);
        }
    }
    let mut bytes = Vec::new();
    file.take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Input)?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Error::Input);
    }
    serde_json::from_value(canonical::parse(&bytes).map_err(|_| Error::Input)?)
        .map_err(|_| Error::Input)
}

/// Reads native stdin through EOF, with a five-second deadline and byte ceiling.
///
/// # Errors
/// Rejects unsupported platforms, stalled input, I/O failure or oversized payloads.
pub fn read_stdin() -> Result<Vec<u8>, Error> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let start = Instant::now();
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let remaining = Duration::from_secs(5)
                .checked_sub(start.elapsed())
                .ok_or(Error::Input)?;
            let mut descriptor = libc::pollfd {
                fd: input.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // The descriptor is live for the entire poll. Only this thread reads stdin.
            let ready =
                unsafe { libc::poll(&mut descriptor, 1, remaining.as_millis().min(5000) as i32) };
            if ready < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            if ready <= 0 {
                return Err(Error::Input);
            }
            let count = match input.read(&mut buffer) {
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Input),
            };
            if count == 0 {
                return Ok(bytes);
            }
            if count > MAX_INPUT_BYTES.saturating_sub(bytes.len()) {
                return Err(Error::Input);
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    #[cfg(not(unix))]
    {
        Err(Error::Input)
    }
}

/// Parses native JSON without duplicate-key loss or AW-only metadata restrictions.
///
/// # Errors
/// Rejects ambiguous JSON, nonfinite numbers, excessive nesting or oversized input.
pub fn parse_payload(bytes: &[u8]) -> Result<Value, Error> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Error::Input);
    }
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let value = NativeValue::deserialize(&mut parser).map_err(|_| Error::Input)?;
    parser.end().map_err(|_| Error::Input)?;
    Ok(value.0)
}

struct NativeValue(Value);
impl<'de> Deserialize<'de> for NativeValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NativeVisitor;
        impl<'de> Visitor<'de> for NativeVisitor {
            type Value = NativeValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unambiguous native JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<NativeValue, E> {
                Ok(NativeValue(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<NativeValue, E> {
                Ok(NativeValue(Value::Null))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<NativeValue, E> {
                Ok(NativeValue(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<NativeValue, E> {
                Ok(NativeValue(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<NativeValue, E> {
                Ok(NativeValue(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<NativeValue, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| NativeValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<NativeValue, A::Error> {
                let mut values = Vec::new();
                while let Some(v) = seq.next_element::<NativeValue>()? {
                    values.push(v.0);
                }
                Ok(NativeValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<NativeValue, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    values.insert(key, map.next_value::<NativeValue>()?.0);
                }
                Ok(NativeValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(NativeVisitor)
    }
}
