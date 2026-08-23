//! Versioned, implementation-neutral contracts for Agent Memory clients and backends.

use schemars::schema_for;
use serde_json::{Value, json};

mod backend;
mod types;

pub use backend::{EphemeralMemoryBackend, MemoryBackend, dispatch};
pub use types::*;

/// Returns the JSON Schema bundle for protocol request and response envelopes.
pub fn schema_bundle() -> Value {
    let request = schema_for!(MemoryRequestEnvelope);
    let response = schema_for!(MemoryWireResponse);
    json!({
        "protocol": MEMORY_PROTOCOL_NAME,
        "version": MEMORY_PROTOCOL_VERSION,
        "request": request,
        "response": response,
    })
}
