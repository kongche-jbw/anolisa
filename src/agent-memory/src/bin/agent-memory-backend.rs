//! JSONL Memory Protocol server used by adapters and backend conformance tests.

use std::io::{self, BufRead, Write};

use agent_memory::protocol::{
    EphemeralMemoryBackend, MemoryRequestEnvelope, MemoryWireResponse, ProtocolError,
    ProtocolErrorCode, dispatch, schema_bundle,
};
use clap::Parser;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const RESPONSE_TOO_LARGE_MESSAGE: &str = "response frame exceeds 1048576 bytes";

#[derive(Debug, Parser)]
#[command(name = "agent-memory-backend", version)]
#[command(about = "Versioned JSONL Agent Memory backend")]
struct Cli {
    /// Print the protocol JSON Schema bundle and exit.
    #[arg(long)]
    schema: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.schema {
        serde_json::to_writer_pretty(io::stdout().lock(), &schema_bundle())?;
        println!();
        return Ok(());
    }

    let backend = EphemeralMemoryBackend::default();
    serve(io::stdin().lock(), io::stdout().lock(), &backend)
}

fn serve<R, W>(mut input: R, mut output: W, backend: &EphemeralMemoryBackend) -> anyhow::Result<()>
where
    R: BufRead,
    W: Write,
{
    loop {
        let mut frame = match read_frame(&mut input)? {
            Frame::Eof => return Ok(()),
            Frame::TooLarge => {
                write_response(
                    &mut output,
                    &MemoryWireResponse::error(
                        "unknown",
                        ProtocolError::new(
                            ProtocolErrorCode::ResourceExhausted,
                            format!("request frame exceeds {MAX_FRAME_BYTES} bytes"),
                            false,
                        ),
                    ),
                )?;
                continue;
            }
            Frame::Data(frame) => frame,
        };
        while matches!(frame.last(), Some(b'\n' | b'\r')) {
            frame.pop();
        }
        if frame.is_empty() {
            continue;
        }
        let response = match serde_json::from_slice::<MemoryRequestEnvelope>(&frame) {
            Ok(request) => dispatch(backend, request),
            Err(error) => decode_error(&frame, &error),
        };
        write_response(&mut output, &response)?;
    }
}

fn decode_error(frame: &[u8], error: &serde_json::Error) -> MemoryWireResponse {
    let value = serde_json::from_slice::<serde_json::Value>(frame).ok();
    let request_id = value
        .as_ref()
        .and_then(|value| value.get("request_id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let operation = value
        .as_ref()
        .and_then(|value| value.pointer("/request/operation"))
        .and_then(serde_json::Value::as_str);
    let known_operation = operation.is_none_or(|operation| {
        matches!(
            operation,
            "negotiate"
                | "open_session"
                | "append_event"
                | "materialize_context"
                | "checkpoint_task"
                | "explain_context"
                | "report_recall_outcome"
                | "forget"
                | "close_session"
        )
    });
    if !known_operation {
        return MemoryWireResponse::error(
            request_id,
            ProtocolError::new(
                ProtocolErrorCode::UnsupportedCapability,
                "request operation is not supported by this protocol version",
                false,
            ),
        );
    }
    MemoryWireResponse::error(
        request_id,
        ProtocolError::new(
            ProtocolErrorCode::InvalidRequest,
            format!("request is not valid protocol JSON: {error}"),
            false,
        ),
    )
}

#[derive(Debug, PartialEq, Eq)]
enum Frame {
    Eof,
    Data(Vec<u8>),
    TooLarge,
}

fn read_frame<R: BufRead>(input: &mut R) -> io::Result<Frame> {
    let mut frame = Vec::with_capacity(4096);
    let mut too_large = false;
    let mut saw_data = false;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return if !saw_data {
                Ok(Frame::Eof)
            } else if too_large {
                Ok(Frame::TooLarge)
            } else {
                Ok(Frame::Data(frame))
            };
        }
        saw_data = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let data_len = newline.unwrap_or(available.len());
        let consumed = newline.map_or(available.len(), |position| position + 1);
        if !too_large {
            if frame.len().saturating_add(data_len) > MAX_FRAME_BYTES {
                too_large = true;
                frame.clear();
            } else {
                frame.extend_from_slice(&available[..data_len]);
            }
        }
        input.consume(consumed);
        if newline.is_some() {
            return if too_large {
                Ok(Frame::TooLarge)
            } else {
                Ok(Frame::Data(frame))
            };
        }
    }
}

fn write_response<W: Write>(output: &mut W, response: &MemoryWireResponse) -> anyhow::Result<()> {
    let frame = match serialize_response(response)? {
        Some(frame) => frame,
        None => {
            let fallback = MemoryWireResponse::error(
                response.request_id(),
                ProtocolError::new(
                    ProtocolErrorCode::ResourceExhausted,
                    RESPONSE_TOO_LARGE_MESSAGE,
                    false,
                ),
            );
            serialize_response(&fallback)?.ok_or_else(|| {
                anyhow::anyhow!("fixed response-too-large frame exceeds protocol limit")
            })?
        }
    };
    output.write_all(&frame)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn serialize_response(response: &MemoryWireResponse) -> anyhow::Result<Option<Vec<u8>>> {
    let mut writer = BoundedWriter::new(MAX_FRAME_BYTES);
    let result = serde_json::to_writer(&mut writer, response);
    if writer.exceeded {
        return Ok(None);
    }
    result?;
    Ok(Some(writer.bytes))
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(4096),
            limit,
            exceeded: false,
        }
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if buf.len() > remaining {
            self.exceeded = true;
            return Err(io::Error::other("response frame exceeds protocol limit"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_response_becomes_bounded_error_and_next_response_is_written() {
        let oversized = MemoryWireResponse::error(
            "oversized-response",
            ProtocolError::new(
                ProtocolErrorCode::Internal,
                "x".repeat(MAX_FRAME_BYTES),
                false,
            ),
        );
        let following = MemoryWireResponse::error(
            "following-response",
            ProtocolError::new(ProtocolErrorCode::InvalidRequest, "expected", false),
        );
        let mut output = Vec::new();

        write_response(&mut output, &oversized).expect("oversized response is replaced");
        write_response(&mut output, &following).expect("following response is still written");

        let lines: Vec<&[u8]> = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].len() <= MAX_FRAME_BYTES);
        assert!(matches!(
            serde_json::from_slice::<MemoryWireResponse>(lines[0])
                .expect("replacement is protocol JSON"),
            MemoryWireResponse::Error {
                request_id,
                error: ProtocolError {
                    code: ProtocolErrorCode::ResourceExhausted,
                    safe_message,
                    retryable: false,
                },
                ..
            } if request_id == "oversized-response"
                && safe_message == RESPONSE_TOO_LARGE_MESSAGE
        ));
        assert!(matches!(
            serde_json::from_slice::<MemoryWireResponse>(lines[1])
                .expect("following response is protocol JSON"),
            MemoryWireResponse::Error { request_id, .. }
                if request_id == "following-response"
        ));
    }
}
