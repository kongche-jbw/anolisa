//! Optional ManT CLI adapter for focused normative knowledge queries.
//!
//! The adapter executes a configured binary directly, negotiates a versioned
//! JSON contract, and never invokes a shell or installs provider software.

use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use super::{
    KnowledgeCapability, KnowledgeError, KnowledgeErrorCode, KnowledgeItem, KnowledgeProvider,
    KnowledgeProviderDescriptor, KnowledgeQuery, KnowledgeResult, KnowledgeSelector,
    floor_char_boundary, now_ms,
};
use crate::protocol::KnowledgeRef;

/// ManT CLI protocol accepted by this adapter revision.
pub const MANT_PROTOCOL: &str = "mant.cli/v0.9";
/// ManT request schema emitted by this adapter revision.
pub const MANT_REQUEST_SCHEMA: &str = "mant.request/v0.9";
/// ManT focused excerpt schema accepted for explain and excerpt queries.
pub const MANT_EXCERPT_SCHEMA: &str = "mant.excerpt/v0.9";
/// ManT focused search schema accepted for search queries.
pub const MANT_SEARCH_SCHEMA: &str = "mant.search/v0.9";
/// Maximum native request size accepted by the ManT v0.9 stdin contract.
pub const MANT_MAX_REQUEST_BYTES: usize = 65_536;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_STDOUT_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_STDERR_BYTES: usize = 8 * 1024;
const MAX_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROCESS_STDOUT_BYTES: usize = 1024 * 1024;
const MAX_PROCESS_STDERR_BYTES: usize = 64 * 1024;
const MAX_TITLE_BYTES: usize = 512;

/// Process and resource policy for an explicitly configured ManT executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MantCliConfig {
    /// Executable path passed directly to [`Command`] without a shell.
    pub executable: PathBuf,
    /// Hard wall-clock limit for each probe or focused query process.
    pub timeout: Duration,
    /// Maximum stdout retained while the pipe is drained.
    pub max_stdout_bytes: usize,
    /// Maximum stderr retained while the pipe is drained.
    pub max_stderr_bytes: usize,
}

impl MantCliConfig {
    /// Creates a bounded configuration for a user-provisioned executable.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            timeout: DEFAULT_TIMEOUT,
            max_stdout_bytes: DEFAULT_MAX_STDOUT_BYTES,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
        }
    }

    fn validate(&self) -> KnowledgeResult<()> {
        if self.executable.as_os_str().is_empty() {
            return Err(KnowledgeError::invalid_request(
                "ManT executable path must not be empty",
            ));
        }
        if self.timeout.is_zero() || self.timeout > MAX_PROCESS_TIMEOUT {
            return Err(KnowledgeError::invalid_request(
                "ManT timeout is outside the adapter boundary",
            ));
        }
        if self.max_stdout_bytes == 0 || self.max_stdout_bytes > MAX_PROCESS_STDOUT_BYTES {
            return Err(KnowledgeError::resource_exhausted(
                "ManT stdout budget is outside the adapter boundary",
            ));
        }
        if self.max_stderr_bytes == 0 || self.max_stderr_bytes > MAX_PROCESS_STDERR_BYTES {
            return Err(KnowledgeError::resource_exhausted(
                "ManT stderr budget is outside the adapter boundary",
            ));
        }
        Ok(())
    }
}

/// Optional provider backed by independent, one-shot ManT CLI processes.
#[derive(Debug, Clone)]
pub struct MantCliProvider {
    config: MantCliConfig,
}

impl MantCliProvider {
    /// Creates an adapter without probing, installing, or downloading ManT.
    pub fn new(config: MantCliConfig) -> Self {
        Self { config }
    }

    fn probe(&self) -> KnowledgeResult<KnowledgeProviderDescriptor> {
        let output = self.run(&["--protocol-version", "--compact"], None)?;
        if !output.status.success() {
            return Err(KnowledgeError::new(
                KnowledgeErrorCode::Incompatible,
                "ManT does not expose the required protocol descriptor",
                false,
            ));
        }
        let probe: MantProbe = serde_json::from_slice(&output.stdout).map_err(|_| {
            KnowledgeError::new(
                KnowledgeErrorCode::MalformedResponse,
                "ManT protocol descriptor is not valid JSON",
                false,
            )
        })?;
        if probe.protocol != MANT_PROTOCOL
            || probe.request_schema != MANT_REQUEST_SCHEMA
            || probe.excerpt_schema != MANT_EXCERPT_SCHEMA
            || probe.search_schema != MANT_SEARCH_SCHEMA
        {
            return Err(KnowledgeError::new(
                KnowledgeErrorCode::Incompatible,
                "ManT protocol descriptor is incompatible with this adapter",
                false,
            ));
        }

        Ok(KnowledgeProviderDescriptor {
            provider_id: "mant".to_owned(),
            display_name: "ManT CLI".to_owned(),
            version: Some(probe.native_api_version),
            protocol: Some(probe.protocol),
            capabilities: vec![
                KnowledgeCapability::Search,
                KnowledgeCapability::Explain,
                KnowledgeCapability::Excerpt,
            ],
        })
    }

    fn request_value(query: &KnowledgeQuery) -> Value {
        let input = json!({
            "kind": "document",
            "selector": query.document_id,
        });
        let view = match &query.selector {
            KnowledgeSelector::Search {
                pattern,
                context_lines,
            } => json!({
                "kind": "search",
                "pattern": pattern,
                "syntax": "literal",
                "case": "insensitive",
                "scope": "visible",
                "word": false,
                "contextLines": context_lines,
                "limit": query.max_items,
                "offset": 0,
            }),
            KnowledgeSelector::Explain { entry } => json!({
                "kind": "explain",
                "entry": entry,
            }),
            KnowledgeSelector::Excerpt { selectors } => json!({
                "kind": "excerpt",
                "selectors": selectors,
            }),
        };
        json!({
            "schema": MANT_REQUEST_SCHEMA,
            "input": input,
            "view": view,
        })
    }

    fn run(&self, arguments: &[&str], input: Option<&[u8]>) -> KnowledgeResult<ProcessOutput> {
        self.config.validate()?;

        let stdin = if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        };
        let deadline = Instant::now() + self.config.timeout;
        let mut command = Command::new(&self.config.executable);
        command
            .args(arguments)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A separate group lets a timeout kill helpers spawned by the CLI.
            .process_group(0);
        let mut child = spawn_with_deadline(&mut command, deadline)?;

        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_group(&mut child);
                return Err(KnowledgeError::new(
                    KnowledgeErrorCode::Internal,
                    "ManT stdout pipe was not created",
                    false,
                ));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_group(&mut child);
                return Err(KnowledgeError::new(
                    KnowledgeErrorCode::Internal,
                    "ManT stderr pipe was not created",
                    false,
                ));
            }
        };
        let stdout_reader = spawn_reader(stdout, self.config.max_stdout_bytes);
        let stderr_reader = spawn_reader(stderr, self.config.max_stderr_bytes);

        let stdin_writer = match input {
            Some(input) => match child.stdin.take() {
                Some(stdin) => Some(spawn_writer(stdin, input.to_vec())),
                None => {
                    terminate_group(&mut child);
                    let _ = collect_reader(stdout_reader);
                    let _ = collect_reader(stderr_reader);
                    return Err(KnowledgeError::new(
                        KnowledgeErrorCode::Internal,
                        "ManT stdin pipe was not created",
                        false,
                    ));
                }
            },
            None => None,
        };

        let status = match wait_with_deadline(&mut child, deadline) {
            Ok(status) => status,
            Err(error) => {
                if let Some(writer) = stdin_writer {
                    let _ = collect_writer(writer);
                }
                let _ = collect_reader(stdout_reader);
                let _ = collect_reader(stderr_reader);
                return Err(error);
            }
        };
        let (stdout, stderr) = collect_process_io_with_deadline(
            &mut child,
            stdin_writer,
            stdout_reader,
            stderr_reader,
            deadline,
        )?;
        if stdout.exceeded || stderr.exceeded {
            return Err(KnowledgeError::resource_exhausted(
                "ManT process output exceeded the adapter boundary",
            ));
        }
        Ok(ProcessOutput {
            status,
            stdout: stdout.bytes,
        })
    }
}

impl KnowledgeProvider for MantCliProvider {
    fn descriptor(&self) -> KnowledgeResult<KnowledgeProviderDescriptor> {
        self.probe()
    }

    fn query(&self, query: &KnowledgeQuery) -> KnowledgeResult<Vec<KnowledgeItem>> {
        query.validate()?;
        let descriptor = self.probe()?;
        if !descriptor
            .capabilities
            .contains(&query.required_capability())
        {
            return Err(KnowledgeError::new(
                KnowledgeErrorCode::Incompatible,
                "ManT does not support the requested focused operation",
                false,
            ));
        }

        let request = serde_json::to_vec(&Self::request_value(query)).map_err(|_| {
            KnowledgeError::new(
                KnowledgeErrorCode::Internal,
                "ManT request serialization failed",
                false,
            )
        })?;
        if request.len() > MANT_MAX_REQUEST_BYTES {
            return Err(KnowledgeError::resource_exhausted(
                "ManT native request exceeds 65536 bytes",
            ));
        }
        let output = self.run(
            &["--request-json", "--format", "json", "--compact"],
            Some(&request),
        )?;
        if !output.status.success() {
            let code = if output.status.code() == Some(2) {
                KnowledgeErrorCode::InvalidRequest
            } else {
                KnowledgeErrorCode::ProviderFailed
            };
            return Err(KnowledgeError::new(
                code,
                "ManT rejected or failed the focused query",
                code == KnowledgeErrorCode::ProviderFailed,
            ));
        }

        let Some((title, excerpt)) = decode_focused_response(&output.stdout, query)? else {
            return Ok(Vec::new());
        };
        let fingerprint = format!("fnv1a64:{:016x}", fnv1a64(&output.stdout));
        Ok(vec![KnowledgeItem {
            reference: KnowledgeRef {
                provider: "mant".to_owned(),
                document_id: query.document_id.clone(),
                selector: Some(query.reference_selector()),
                content_digest: Some(fingerprint.clone()),
                retrieved_at_ms: now_ms(),
            },
            title: Some(title),
            excerpt,
            fingerprint,
            score: None,
        }])
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MantProbe {
    protocol: String,
    native_api_version: String,
    request_schema: String,
    excerpt_schema: String,
    search_schema: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MantSearchResponse {
    schema: String,
    label: String,
    query: MantSearchQuery,
    render: MantSearchRender,
    total: u32,
    returned: u32,
    offset: u32,
    truncated: bool,
    matches: Vec<MantSearchHit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MantSearchQuery {
    pattern: String,
    syntax: String,
    case: String,
    scope: String,
    word: bool,
    context_lines: u8,
    limit: u32,
    offset: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MantSearchRender {
    schema: String,
    format: String,
    scope: String,
    line_base: u32,
    column_base: u32,
    line_count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MantSearchHit {
    ordinal: u32,
    outline: MantOutlineTrail,
    occurrences: Vec<MantSearchOccurrence>,
    occurrence_count: u32,
    occurrences_truncated: bool,
    preview: String,
    #[serde(default)]
    context: Vec<MantSearchContextLine>,
    #[serde(default)]
    node_source: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MantSearchOccurrence {
    matched_text: String,
    markdown: MantMarkdownRange,
    line_ranges: Vec<MantLineRange>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MantMarkdownRange {
    start_byte: u64,
    end_byte: u64,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MantLineRange {
    line: u32,
    start_byte: u32,
    end_byte: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MantSearchContextLine {
    line: u32,
    text: String,
    matched: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MantOutlineTrail {
    node: MantOutlineNode,
    #[serde(default)]
    ancestors: Vec<MantOutlineReference>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum MantOutlineNode {
    Tldr {
        path: String,
        id: String,
        title: String,
    },
    DocumentRoot {
        path: String,
        id: String,
        title: String,
    },
    DocumentSection {
        path: String,
        id: String,
        title: String,
    },
    DocumentEntry {
        path: String,
        id: String,
        title: String,
        role: String,
        case: String,
        names: Vec<String>,
    },
}

#[derive(Debug, Deserialize)]
struct MantOutlineReference {
    path: String,
    id: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct MantExcerptResponse {
    schema: String,
    label: String,
    selections: Vec<MantExcerptSelection>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum MantExcerptSelection {
    Tldr {
        outline: MantOutlineTrail,
        document: MantTldrDocument,
    },
    DocumentRoot {
        outline: MantOutlineTrail,
        blocks: Vec<MantBlock>,
    },
    DocumentSection {
        outline: MantOutlineTrail,
        section: MantSection,
    },
    DocumentEntry {
        outline: MantOutlineTrail,
        entry: MantDefinitionItem,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum MantInline {
    Text {
        value: String,
    },
    Strong {
        children: Vec<MantInline>,
    },
    Emphasis {
        children: Vec<MantInline>,
    },
    Code {
        value: String,
    },
    Link {
        target: MantLinkTarget,
        children: Vec<MantInline>,
    },
    Anchor {
        id: String,
    },
    LineBreak,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum MantLinkTarget {
    External { uri: String },
    Email { address: String },
    Document { name: String },
    Manual { name: String },
    Section { id: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum MantBlock {
    Paragraph {
        children: Vec<MantInline>,
    },
    Preformatted {
        children: Vec<MantInline>,
    },
    List {
        kind: String,
        items: Vec<MantListItem>,
    },
    DefinitionList {
        items: Vec<MantDefinitionItem>,
    },
    Table {
        rows: Vec<MantTableRow>,
    },
    Equation {
        value: String,
    },
    VerticalSpace {
        lines: u16,
    },
    ThematicBreak,
    Unsupported {
        text: String,
    },
}

#[derive(Debug, Deserialize)]
struct MantListItem {
    blocks: Vec<MantBlock>,
}

#[derive(Debug, Deserialize)]
struct MantTableRow {
    cells: Vec<MantTableCell>,
}

#[derive(Debug, Deserialize)]
struct MantTableCell {
    blocks: Vec<MantBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MantDefinitionItem {
    terms: Vec<Vec<MantInline>>,
    description: Vec<MantBlock>,
}

#[derive(Debug, Deserialize)]
struct MantSection {
    id: String,
    title: String,
    blocks: Vec<MantBlock>,
    children: Vec<MantSection>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MantTldrDocument {
    title: String,
    description: Vec<String>,
    examples: Vec<MantTldrExample>,
    platform: String,
    language: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MantTldrExample {
    description: String,
    command: String,
    command_parts: Vec<MantTldrCommandPart>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum MantTldrCommandPart {
    Text { value: String },
    Placeholder { value: String },
}

struct ProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
}

struct BoundedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
}

fn spawn_writer<W>(mut writer: W, bytes: Vec<u8>) -> JoinHandle<io::Result<()>>
where
    W: Write + Send + 'static,
{
    thread::spawn(move || writer.write_all(&bytes))
}

fn collect_writer(writer: JoinHandle<io::Result<()>>) -> KnowledgeResult<()> {
    writer
        .join()
        .map_err(|_| {
            KnowledgeError::new(
                KnowledgeErrorCode::Internal,
                "ManT request writer terminated unexpectedly",
                false,
            )
        })?
        .map_err(|_| {
            KnowledgeError::new(
                KnowledgeErrorCode::ProviderFailed,
                "ManT closed its request stream",
                true,
            )
        })
}

fn spawn_reader<R>(reader: R, maximum: usize) -> JoinHandle<io::Result<BoundedOutput>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_bounded(reader, maximum))
}

fn read_bounded(mut reader: impl Read, maximum: usize) -> io::Result<BoundedOutput> {
    let mut bytes = Vec::with_capacity(maximum.min(8 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    let mut exceeded = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = maximum.saturating_sub(bytes.len());
        let retained = count.min(remaining);
        bytes.extend_from_slice(&buffer[..retained]);
        exceeded |= retained < count;
    }
    Ok(BoundedOutput { bytes, exceeded })
}

fn collect_reader(reader: JoinHandle<io::Result<BoundedOutput>>) -> KnowledgeResult<BoundedOutput> {
    reader
        .join()
        .map_err(|_| {
            KnowledgeError::new(
                KnowledgeErrorCode::Internal,
                "ManT output reader terminated unexpectedly",
                false,
            )
        })?
        .map_err(|_| {
            KnowledgeError::new(
                KnowledgeErrorCode::Internal,
                "ManT output stream could not be read",
                true,
            )
        })
}

fn wait_with_deadline(child: &mut Child, deadline: Instant) -> KnowledgeResult<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                thread::sleep(remaining.min(Duration::from_millis(5)));
            }
            Ok(None) => {
                terminate_group(child);
                return Err(KnowledgeError::new(
                    KnowledgeErrorCode::Timeout,
                    "ManT process exceeded its execution deadline",
                    true,
                ));
            }
            Err(_) => {
                terminate_group(child);
                return Err(KnowledgeError::new(
                    KnowledgeErrorCode::ProviderFailed,
                    "ManT process status could not be observed",
                    true,
                ));
            }
        }
    }
}

fn collect_process_io_with_deadline(
    child: &mut Child,
    stdin: Option<JoinHandle<io::Result<()>>>,
    stdout: JoinHandle<io::Result<BoundedOutput>>,
    stderr: JoinHandle<io::Result<BoundedOutput>>,
    deadline: Instant,
) -> KnowledgeResult<(BoundedOutput, BoundedOutput)> {
    while stdin.as_ref().is_some_and(|writer| !writer.is_finished())
        || !stdout.is_finished()
        || !stderr.is_finished()
    {
        if Instant::now() >= deadline {
            terminate_group(child);
            if let Some(writer) = stdin {
                let _ = collect_writer(writer);
            }
            let _ = collect_reader(stdout);
            let _ = collect_reader(stderr);
            return Err(KnowledgeError::new(
                KnowledgeErrorCode::Timeout,
                "ManT process I/O exceeded the execution deadline",
                true,
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::sleep(remaining.min(Duration::from_millis(5)));
    }
    if let Some(writer) = stdin {
        collect_writer(writer)?;
    }
    Ok((collect_reader(stdout)?, collect_reader(stderr)?))
}

fn terminate_group(child: &mut Child) {
    let group_killed = i32::try_from(child.id())
        .ok()
        .map(Pid::from_raw)
        .is_some_and(|group| killpg(group, Signal::SIGKILL).is_ok());
    if !group_killed {
        // A raced process exit is harmless; wait still reaps the child.
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn map_spawn_error(error: &io::Error) -> KnowledgeError {
    let code = if error.kind() == io::ErrorKind::NotFound
        || error.kind() == io::ErrorKind::PermissionDenied
    {
        KnowledgeErrorCode::Unavailable
    } else {
        KnowledgeErrorCode::ProviderFailed
    };
    KnowledgeError::new(code, "ManT executable could not be started", true)
}

fn spawn_with_deadline(command: &mut Command, deadline: Instant) -> KnowledgeResult<Child> {
    loop {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) && Instant::now() < deadline =>
            {
                let remaining = deadline.saturating_duration_since(Instant::now());
                thread::sleep(remaining.min(Duration::from_millis(5)));
            }
            Err(error) => return Err(map_spawn_error(&error)),
        }
    }
}

fn decode_focused_response(
    bytes: &[u8],
    query: &KnowledgeQuery,
) -> KnowledgeResult<Option<(String, String)>> {
    match &query.selector {
        KnowledgeSelector::Search {
            pattern,
            context_lines,
        } => decode_search_response(bytes, query, pattern, *context_lines),
        KnowledgeSelector::Explain { .. } | KnowledgeSelector::Excerpt { .. } => {
            decode_excerpt_response(bytes, query)
        }
    }
}

fn decode_search_response(
    bytes: &[u8],
    request: &KnowledgeQuery,
    pattern: &str,
    context_lines: u8,
) -> KnowledgeResult<Option<(String, String)>> {
    let response: MantSearchResponse = serde_json::from_slice(bytes)
        .map_err(|_| malformed_response("ManT search response is not valid v0.9 JSON"))?;
    if response.schema != MANT_SEARCH_SCHEMA
        || response.label.is_empty()
        || response.query.pattern != pattern
        || response.query.syntax != "literal"
        || response.query.case != "insensitive"
        || response.query.scope != "visible"
        || response.query.word
        || response.query.context_lines != context_lines
        || response.query.limit != u32::from(request.max_items)
        || response.query.offset != 0
        || response.render.schema != "mant.markdown/v1"
        || response.render.format != "markdown"
        || response.render.scope != "full"
        || response.render.line_base != 1
        || response.render.column_base != 1
        || response.offset != 0
        || response.returned as usize != response.matches.len()
        || response.total < response.returned
        || response.returned > u32::from(request.max_items)
        || response.truncated != (response.total > response.offset + response.returned)
    {
        return Err(malformed_response(
            "ManT search response violates the v0.9 contract",
        ));
    }
    if response.matches.is_empty() {
        return Ok(None);
    }
    if response.render.line_count == 0 {
        return Err(malformed_response(
            "ManT search response has matches without rendered lines",
        ));
    }

    let mut excerpt = String::new();
    for hit in &response.matches {
        validate_search_hit(hit)?;
        append_bounded(&hit.preview, &mut excerpt, request.max_excerpt_bytes);
        if excerpt.len() >= request.max_excerpt_bytes {
            break;
        }
    }
    if excerpt.is_empty() {
        return Err(malformed_response(
            "ManT search response contains no focused preview",
        ));
    }
    Ok(Some((bounded_label(&response.label), excerpt)))
}

fn validate_search_hit(hit: &MantSearchHit) -> KnowledgeResult<()> {
    validate_outline(&hit.outline)?;
    if hit.ordinal == 0
        || hit.occurrences.is_empty()
        || hit.occurrences.len() > 256
        || hit.occurrence_count < hit.occurrences.len() as u32
        || (!hit.occurrences_truncated && hit.occurrence_count != hit.occurrences.len() as u32)
    {
        return Err(malformed_response(
            "ManT search hit violates the v0.9 contract",
        ));
    }
    for occurrence in &hit.occurrences {
        let range = &occurrence.markdown;
        if occurrence.matched_text.is_empty()
            || occurrence.line_ranges.is_empty()
            || range.start_byte > range.end_byte
            || range.start_line == 0
            || range.start_column == 0
            || range.end_line == 0
            || range.end_column == 0
            || (range.start_line, range.start_column) > (range.end_line, range.end_column)
            || occurrence
                .line_ranges
                .iter()
                .any(|line| line.line == 0 || line.start_byte > line.end_byte)
        {
            return Err(malformed_response(
                "ManT search occurrence violates the v0.9 contract",
            ));
        }
    }
    if hit
        .context
        .iter()
        .any(|line| line.line == 0 || (line.matched && line.text.is_empty()))
    {
        return Err(malformed_response(
            "ManT search context violates the v0.9 contract",
        ));
    }
    if hit
        .node_source
        .as_ref()
        .is_some_and(|source| !source.is_object())
    {
        return Err(malformed_response(
            "ManT search source violates the v0.9 contract",
        ));
    }
    Ok(())
}

fn decode_excerpt_response(
    bytes: &[u8],
    query: &KnowledgeQuery,
) -> KnowledgeResult<Option<(String, String)>> {
    let response: MantExcerptResponse = serde_json::from_slice(bytes)
        .map_err(|_| malformed_response("ManT excerpt response is not valid v0.9 JSON"))?;
    if response.schema != MANT_EXCERPT_SCHEMA || response.label.is_empty() {
        return Err(malformed_response(
            "ManT excerpt response violates the v0.9 contract",
        ));
    }
    if response.selections.is_empty() {
        return Ok(None);
    }

    let mut excerpt = String::new();
    for selection in &response.selections {
        validate_and_append_selection(selection, query, &mut excerpt)?;
        if excerpt.len() >= query.max_excerpt_bytes {
            break;
        }
    }
    if excerpt.is_empty() {
        return Err(malformed_response(
            "ManT excerpt response contains no focused content",
        ));
    }
    Ok(Some((bounded_label(&response.label), excerpt)))
}

fn validate_and_append_selection(
    selection: &MantExcerptSelection,
    query: &KnowledgeQuery,
    excerpt: &mut String,
) -> KnowledgeResult<()> {
    let maximum = query.max_excerpt_bytes;
    match selection {
        MantExcerptSelection::Tldr { outline, document } => {
            reject_non_entry_explain(query)?;
            validate_outline(outline)?;
            require_outline_kind(outline, "tldr")?;
            document.append_visible(excerpt, maximum);
        }
        MantExcerptSelection::DocumentRoot { outline, blocks } => {
            reject_non_entry_explain(query)?;
            validate_outline(outline)?;
            require_outline_kind(outline, "document-root")?;
            append_blocks(blocks, excerpt, maximum);
        }
        MantExcerptSelection::DocumentSection { outline, section } => {
            reject_non_entry_explain(query)?;
            validate_outline(outline)?;
            require_outline_kind(outline, "document-section")?;
            section.append_visible(excerpt, maximum);
        }
        MantExcerptSelection::DocumentEntry { outline, entry } => {
            validate_outline(outline)?;
            require_outline_kind(outline, "document-entry")?;
            entry.append_visible(excerpt, maximum);
        }
    }
    Ok(())
}

fn reject_non_entry_explain(query: &KnowledgeQuery) -> KnowledgeResult<()> {
    if matches!(query.selector, KnowledgeSelector::Explain { .. }) {
        return Err(malformed_response(
            "ManT explain response is not a document entry",
        ));
    }
    Ok(())
}

fn validate_outline(outline: &MantOutlineTrail) -> KnowledgeResult<()> {
    outline.node.validate()?;
    for ancestor in &outline.ancestors {
        if ancestor.path.is_empty() || ancestor.id.is_empty() || ancestor.title.is_empty() {
            return Err(malformed_response(
                "ManT outline ancestor violates the v0.9 contract",
            ));
        }
    }
    Ok(())
}

fn require_outline_kind(outline: &MantOutlineTrail, expected: &str) -> KnowledgeResult<()> {
    if outline.node.kind() != expected {
        return Err(malformed_response(
            "ManT selection and outline kinds do not match",
        ));
    }
    Ok(())
}

impl MantOutlineNode {
    fn kind(&self) -> &'static str {
        match self {
            Self::Tldr { .. } => "tldr",
            Self::DocumentRoot { .. } => "document-root",
            Self::DocumentSection { .. } => "document-section",
            Self::DocumentEntry { .. } => "document-entry",
        }
    }

    fn validate(&self) -> KnowledgeResult<()> {
        let (path, id, title) = match self {
            Self::Tldr { path, id, title }
            | Self::DocumentRoot { path, id, title }
            | Self::DocumentSection { path, id, title } => (path, id, title),
            Self::DocumentEntry {
                path,
                id,
                title,
                role,
                case,
                names,
            } => {
                if !matches!(
                    role.as_str(),
                    "option" | "command" | "environment-variable" | "variable"
                ) || !matches!(case.as_str(), "sensitive" | "insensitive")
                    || names.iter().any(String::is_empty)
                {
                    return Err(malformed_response(
                        "ManT entry outline violates the v0.9 contract",
                    ));
                }
                (path, id, title)
            }
        };
        if path.is_empty() || id.is_empty() || title.is_empty() {
            return Err(malformed_response(
                "ManT outline node violates the v0.9 contract",
            ));
        }
        Ok(())
    }
}

impl MantInline {
    fn append_visible(&self, output: &mut String, maximum: usize) {
        match self {
            Self::Text { value } | Self::Code { value } => {
                append_bounded(value, output, maximum);
            }
            Self::Strong { children } | Self::Emphasis { children } => {
                append_inlines(children, output, maximum);
            }
            Self::Link { target, children } => {
                target.observe();
                append_inlines(children, output, maximum);
            }
            Self::Anchor { id } => {
                let _ = id;
            }
            Self::LineBreak => {}
        }
    }
}

impl MantLinkTarget {
    fn observe(&self) {
        match self {
            Self::External { uri } => {
                let _ = uri;
            }
            Self::Email { address } => {
                let _ = address;
            }
            Self::Document { name } | Self::Manual { name } => {
                let _ = name;
            }
            Self::Section { id } => {
                let _ = id;
            }
        }
    }
}

impl MantBlock {
    fn append_visible(&self, output: &mut String, maximum: usize) {
        match self {
            Self::Paragraph { children } | Self::Preformatted { children } => {
                append_inlines(children, output, maximum);
            }
            Self::List { kind, items } => {
                let _ = kind;
                for item in items {
                    append_blocks(&item.blocks, output, maximum);
                }
            }
            Self::DefinitionList { items } => {
                for item in items {
                    item.append_visible(output, maximum);
                }
            }
            Self::Table { rows } => {
                for row in rows {
                    for cell in &row.cells {
                        append_blocks(&cell.blocks, output, maximum);
                    }
                }
            }
            Self::Equation { value } | Self::Unsupported { text: value } => {
                append_bounded(value, output, maximum);
            }
            Self::VerticalSpace { lines } => {
                let _ = lines;
            }
            Self::ThematicBreak => {}
        }
    }
}

impl MantDefinitionItem {
    fn append_visible(&self, output: &mut String, maximum: usize) {
        for term in &self.terms {
            append_inlines(term, output, maximum);
            if output.len() >= maximum {
                return;
            }
        }
        append_blocks(&self.description, output, maximum);
    }
}

impl MantSection {
    fn append_visible(&self, output: &mut String, maximum: usize) {
        let _ = &self.id;
        append_bounded(&self.title, output, maximum);
        append_blocks(&self.blocks, output, maximum);
        for child in &self.children {
            if output.len() >= maximum {
                return;
            }
            child.append_visible(output, maximum);
        }
    }
}

impl MantTldrDocument {
    fn append_visible(&self, output: &mut String, maximum: usize) {
        append_bounded(&self.title, output, maximum);
        for paragraph in &self.description {
            append_bounded(paragraph, output, maximum);
        }
        for example in &self.examples {
            append_bounded(&example.description, output, maximum);
            append_bounded(&example.command, output, maximum);
            for part in &example.command_parts {
                match part {
                    MantTldrCommandPart::Text { value }
                    | MantTldrCommandPart::Placeholder { value } => {
                        let _ = value;
                    }
                }
            }
        }
        let _ = (&self.platform, &self.language);
    }
}

fn append_inlines(values: &[MantInline], output: &mut String, maximum: usize) {
    for value in values {
        value.append_visible(output, maximum);
        if output.len() >= maximum {
            return;
        }
    }
}

fn append_blocks(values: &[MantBlock], output: &mut String, maximum: usize) {
    for value in values {
        value.append_visible(output, maximum);
        if output.len() >= maximum {
            return;
        }
    }
}

fn malformed_response(message: &'static str) -> KnowledgeError {
    KnowledgeError::new(KnowledgeErrorCode::MalformedResponse, message, false)
}

fn append_bounded(value: &str, output: &mut String, maximum: usize) {
    if value.is_empty() || output.len() >= maximum {
        return;
    }
    if !output.is_empty() {
        if output.len() + 1 > maximum {
            return;
        }
        output.push('\n');
    }
    let remaining = maximum.saturating_sub(output.len());
    let boundary = floor_char_boundary(value, remaining);
    output.push_str(&value[..boundary]);
}

fn bounded_label(label: &str) -> String {
    let boundary = floor_char_boundary(label, MAX_TITLE_BYTES);
    label[..boundary].to_owned()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
