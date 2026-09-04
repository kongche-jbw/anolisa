//! Provider package fixtures shared by the Core plan tests.
//!
//! Each fixture writes a real manifest, real schema resources with matching
//! digests, and a shell script that returns a fixed native response, so the
//! tests exercise genuine admission and codec mapping rather than a mock.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use aw_contracts::context::{
    CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_SHA256 as PROJECTION_INPUT_SHA256,
    CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_SHA256 as PROJECTION_OUTPUT_SHA256,
};
use aw_contracts::security::{
    SECURITY_CODE_INSPECT_INPUT_SCHEMA_SHA256 as CODE_INPUT_SHA256,
    SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_SHA256 as CODE_OUTPUT_SHA256,
    SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_SHA256 as COMMAND_INPUT_SHA256,
    SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_SHA256 as COMMAND_OUTPUT_SHA256,
    SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_SHA256 as CONTENT_INPUT_SHA256,
    SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_SHA256 as CONTENT_OUTPUT_SHA256,
};

const PROJECTION_INPUT_SCHEMA: &str =
    include_str!("../../../aw-contracts/schemas/context-projection-prepare-input-v1.schema.json");
const PROJECTION_OUTPUT_SCHEMA: &str =
    include_str!("../../../aw-contracts/schemas/context-projection-prepare-output-v1.schema.json");
const CONTENT_INPUT_SCHEMA: &str =
    include_str!("../../../aw-contracts/schemas/security-content-inspect-input-v1.schema.json");
const CONTENT_OUTPUT_SCHEMA: &str =
    include_str!("../../../aw-contracts/schemas/security-content-inspect-output-v1.schema.json");
const CODE_INPUT_SCHEMA: &str =
    include_str!("../../../aw-contracts/schemas/security-code-inspect-input-v1.schema.json");
const CODE_OUTPUT_SCHEMA: &str =
    include_str!("../../../aw-contracts/schemas/security-code-inspect-output-v1.schema.json");
const COMMAND_INPUT_SCHEMA: &str =
    include_str!("../../../aw-contracts/schemas/security-command-inspect-input-v1.schema.json");
const COMMAND_OUTPUT_SCHEMA: &str =
    include_str!("../../../aw-contracts/schemas/security-command-inspect-output-v1.schema.json");

const EMPTY_SCHEMA_SHA256: &str =
    "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";

/// Which Capability shape a fixture Provider package implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixtureKind {
    /// Advise context projection that produces a lossless candidate.
    Projection,
    /// Observe content inspection that reports one credential finding.
    ContentInspect,
    /// Observe content inspection that reports a verified partial finding.
    ContentInspectPartial,
    /// Observe content inspection that falsely claims complete zero-byte coverage.
    ContentInspectWrongCoverage,
    /// Observe code inspection that reports one dangerous-pattern finding.
    CodeInspect,
    /// Observe content inspection whose process settles as an error.
    ContentInspectFailing,
    /// Observe content inspection that returns a matched value it must not.
    ContentInspectLeaking,
    /// Mediate command inspection that allows the pending Tool Call.
    CommandInspectAllow,
    /// Mediate command inspection that falsely allows without scanning the input.
    CommandInspectWrongCoverage,
    /// Mediate command inspection that denies the pending Tool Call.
    ///
    /// A deny verdict is fixture behaviour. The rules that ship with
    /// agent-sec-core only reach `warn`, so the `Block` gate needs a Provider
    /// that states `deny` for the mapping itself to be under test.
    CommandInspectDeny,
    /// Mediate command inspection whose process settles as an error.
    CommandInspectFailing,
}

/// Writes one fixture Provider package below `root`.
pub(crate) fn write_provider(root: &Path, provider_id: &str, kind: FixtureKind) {
    let package = root.join(provider_id);
    fs::create_dir(&package).expect("Provider package directory is created");
    fs::write(package.join("native.schema.json"), "{}").expect("native schema is written");

    let (input_schema, output_schema) = match kind {
        FixtureKind::Projection => (PROJECTION_INPUT_SCHEMA, PROJECTION_OUTPUT_SCHEMA),
        FixtureKind::CodeInspect => (CODE_INPUT_SCHEMA, CODE_OUTPUT_SCHEMA),
        FixtureKind::CommandInspectAllow
        | FixtureKind::CommandInspectWrongCoverage
        | FixtureKind::CommandInspectDeny
        | FixtureKind::CommandInspectFailing => (COMMAND_INPUT_SCHEMA, COMMAND_OUTPUT_SCHEMA),
        FixtureKind::ContentInspect
        | FixtureKind::ContentInspectPartial
        | FixtureKind::ContentInspectWrongCoverage
        | FixtureKind::ContentInspectFailing
        | FixtureKind::ContentInspectLeaking => (CONTENT_INPUT_SCHEMA, CONTENT_OUTPUT_SCHEMA),
    };
    fs::write(package.join("input.schema.json"), input_schema).expect("input schema is written");
    fs::write(package.join("output.schema.json"), output_schema).expect("output schema is written");

    let executable = package.join("fake-provider.sh");
    fs::write(&executable, script(kind)).expect("fixture executable is written");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("fixture executable is made executable");
    fs::write(package.join("provider.toml"), manifest(provider_id, kind))
        .expect("fixture manifest is written");
}

fn script(kind: FixtureKind) -> String {
    let response = match kind {
        FixtureKind::Projection => {
            r#"{"disposition":"applied","output":"projected output"}"#.to_owned()
        }
        FixtureKind::ContentInspect => concat!(
            r#"{"disposition":"completed","verdict":"sensitive","#,
            r#""findings":[{"rule_id":"fixture.private_key","category":"credential","#,
            r#""severity":"high","confidence":"high","count":2}],"#,
            r#""scanned_bytes":__SCANNED_BYTES__,"truncated":false}"#
        )
        .to_owned(),
        FixtureKind::ContentInspectPartial => concat!(
            r#"{"disposition":"completed","verdict":"suspicious","#,
            r#""findings":[{"rule_id":"fixture.partial","category":"dangerous_pattern","#,
            r#""severity":"medium","confidence":"high","count":1}],"#,
            r#""scanned_bytes":3,"truncated":true}"#
        )
        .to_owned(),
        FixtureKind::ContentInspectWrongCoverage => concat!(
            r#"{"disposition":"completed","verdict":"suspicious","#,
            r#""findings":[{"rule_id":"fixture.zero_coverage","category":"dangerous_pattern","#,
            r#""severity":"medium","confidence":"high","count":1}],"#,
            r#""scanned_bytes":0,"truncated":false}"#
        )
        .to_owned(),
        FixtureKind::CodeInspect => concat!(
            r#"{"disposition":"completed","verdict":"suspicious","#,
            r#""findings":[{"rule_id":"fixture.download_exec","category":"dangerous_pattern","#,
            r#""severity":"medium","confidence":"high","count":1}],"#,
            r#""scanned_bytes":__SCANNED_BYTES__,"truncated":false,"language_detected":"bash"}"#
        )
        .to_owned(),
        FixtureKind::ContentInspectFailing => {
            r#"{"disposition":"error","scanned_bytes":0}"#.to_owned()
        }
        FixtureKind::CommandInspectAllow => concat!(
            r#"{"disposition":"completed","verdict":"allow","#,
            r#""reasons":[],"findings":[],"scanned_bytes":__SCANNED_BYTES__}"#
        )
        .to_owned(),
        FixtureKind::CommandInspectWrongCoverage => concat!(
            r#"{"disposition":"completed","verdict":"allow","#,
            r#""reasons":[],"findings":[],"scanned_bytes":0}"#
        )
        .to_owned(),
        // The reason list carries codes only. A gate notice rendered from this
        // response cannot echo the command it refused.
        FixtureKind::CommandInspectDeny => concat!(
            r#"{"disposition":"completed","verdict":"deny","#,
            r#""reasons":["fixture.recursive_delete"],"#,
            r#""findings":[{"rule_id":"fixture.recursive_delete","#,
            r#""category":"dangerous_pattern","severity":"critical","#,
            r#""confidence":"high","count":1}],"scanned_bytes":__SCANNED_BYTES__}"#
        )
        .to_owned(),
        FixtureKind::CommandInspectFailing => {
            r#"{"disposition":"error","scanned_bytes":0}"#.to_owned()
        }
        // A finding must never carry the value it matched. The canonical output
        // schema and the typed Contract both forbid the extra field, so Core
        // has to reject this rather than pass it on.
        FixtureKind::ContentInspectLeaking => concat!(
            r#"{"disposition":"completed","verdict":"sensitive","#,
            r#""findings":[{"rule_id":"fixture.private_key","category":"credential","#,
            r#""severity":"high","confidence":"high","count":1,"#,
            r#""match":"LTAI5tFixtureLeakedSecret"}],"#,
            r#""scanned_bytes":42,"truncated":false}"#
        )
        .to_owned(),
    };
    if response.contains("__SCANNED_BYTES__") {
        format!(
            r#"#!/bin/sh
IFS= read -r payload || true
value=${{payload#*:\"}}
value=${{value%??}}
scanned_bytes=$(printf '%s' "$value" | wc -c | tr -d ' ')
printf '%s' '{response}' | sed "s/__SCANNED_BYTES__/$scanned_bytes/"
"#
        )
    } else {
        format!("#!/bin/sh\nIFS= read -r payload || true\nprintf '%s' '{response}'\n")
    }
}

fn manifest(provider_id: &str, kind: FixtureKind) -> String {
    let header = format!(
        r#"api_version = "providers.agentic-os.sh/v1"
provider_id = "{provider_id}"
provider_version = "1.0.0"
driver = "exec-json/v1"
lifecycle = "one_shot"

[executable]
command = "./fake-provider.sh"
args = []

[limits]
# Generous on purpose. This ceiling only bounds a runaway fixture; a loaded
# runner can take over a second just to spawn the script, and a timeout there
# would report provider_timeout instead of the mapping under test. Deadline
# behaviour is covered in aw-provider-host with a deliberately slow fixture.
wall_time_ms = 30000
input_bytes = 1048576
output_bytes = 1048576

[permissions]
network = "none"
inherit_environment = false
filesystem_read = []
filesystem_write = []

[data]
reads = ["model_visible_context"]
writes = []
sensitivity = "inherits_input"
retention = "none"
telemetry = "disabled"
"#
    );
    format!("{header}{}", capability(kind))
}

fn capability(kind: FixtureKind) -> String {
    match kind {
        FixtureKind::Projection => format!(
            r#"
[[capabilities]]
capability = "context.projection.prepare/v1"
authority = "advise"
scopes = ["tool_call"]
input_contract = {{ schema = "context.projection.prepare.input/v1", resource = "input.schema.json", sha256 = "{PROJECTION_INPUT_SHA256}" }}
output_contract = {{ schema = "context.projection.prepare.output/v1", resource = "output.schema.json", sha256 = "{PROJECTION_OUTPUT_SHA256}" }}
native_input = {{ resource = "native.schema.json", sha256 = "{EMPTY_SCHEMA_SHA256}" }}
native_output = {{ resource = "native.schema.json", sha256 = "{EMPTY_SCHEMA_SHA256}" }}

[capabilities.codec]
kind = "json-map/v1"

[[capabilities.codec.request.fields]]
target = "/content"
source = {{ kind = "input", pointer = "/artifact/content" }}
on_missing = "reject"

[capabilities.codec.response.disposition]
source = "/disposition"
on_unknown = "fail"

[capabilities.codec.response.disposition.values]
applied = "produced"

[[capabilities.codec.response.output_fields]]
target = "/candidate/source_artifact_id"
source = {{ kind = "input", pointer = "/artifact/id" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/candidate/source_digest"
source = {{ kind = "input", pointer = "/artifact/digest" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/candidate/content"
source = {{ kind = "response", pointer = "/output" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/candidate/media_type"
source = {{ kind = "const", value = "text/plain" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/candidate/transform_chain"
source = {{ kind = "const", value = ["fixture"] }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/candidate/reversibility"
source = {{ kind = "const", value = "lossless" }}
when_disposition = ["produced"]
"#
        ),
        FixtureKind::CodeInspect => format!(
            r#"
[[capabilities]]
capability = "security.code.inspect/v1"
authority = "observe"
scopes = ["tool_call"]
input_contract = {{ schema = "security.code.inspect.input/v1", resource = "input.schema.json", sha256 = "{CODE_INPUT_SHA256}" }}
output_contract = {{ schema = "security.code.inspect.output/v1", resource = "output.schema.json", sha256 = "{CODE_OUTPUT_SHA256}" }}
native_input = {{ resource = "native.schema.json", sha256 = "{EMPTY_SCHEMA_SHA256}" }}
native_output = {{ resource = "native.schema.json", sha256 = "{EMPTY_SCHEMA_SHA256}" }}

[capabilities.codec]
kind = "json-map/v1"

[[capabilities.codec.request.fields]]
target = "/content"
source = {{ kind = "input", pointer = "/artifact/content" }}
on_missing = "reject"

[capabilities.codec.response.disposition]
source = "/disposition"
on_unknown = "fail"

[capabilities.codec.response.disposition.values]
completed = "produced"
error = "failed"

[[capabilities.codec.response.output_fields]]
target = "/inspection/verdict"
source = {{ kind = "response", pointer = "/verdict" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/inspection/findings"
source = {{ kind = "response", pointer = "/findings" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/inspection/scanned_bytes"
source = {{ kind = "response", pointer = "/scanned_bytes" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/inspection/truncated"
source = {{ kind = "response", pointer = "/truncated" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/inspection/language_detected"
source = {{ kind = "response", pointer = "/language_detected" }}
when_disposition = ["produced"]

[[capabilities.codec.response.meters]]
meter_id = "security.scanned_bytes"
unit = "bytes"
measurement_kind = "observed"
value_pointer = "/scanned_bytes"
"#
        ),
        FixtureKind::CommandInspectAllow
        | FixtureKind::CommandInspectWrongCoverage
        | FixtureKind::CommandInspectDeny
        | FixtureKind::CommandInspectFailing => format!(
            r#"
[[capabilities]]
capability = "security.command.inspect/v1"
authority = "mediate"
scopes = ["tool_call"]
input_contract = {{ schema = "security.command.inspect.input/v1", resource = "input.schema.json", sha256 = "{COMMAND_INPUT_SHA256}" }}
output_contract = {{ schema = "security.command.inspect.output/v1", resource = "output.schema.json", sha256 = "{COMMAND_OUTPUT_SHA256}" }}
native_input = {{ resource = "native.schema.json", sha256 = "{EMPTY_SCHEMA_SHA256}" }}
native_output = {{ resource = "native.schema.json", sha256 = "{EMPTY_SCHEMA_SHA256}" }}

[capabilities.codec]
kind = "json-map/v1"

[[capabilities.codec.request.fields]]
target = "/command"
source = {{ kind = "input", pointer = "/command/content" }}
on_missing = "reject"

[capabilities.codec.response.disposition]
source = "/disposition"
on_unknown = "fail"

[capabilities.codec.response.disposition.values]
completed = "produced"
error = "failed"

[[capabilities.codec.response.output_fields]]
target = "/decision/verdict"
source = {{ kind = "response", pointer = "/verdict" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/decision/reasons"
source = {{ kind = "response", pointer = "/reasons" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/decision/findings"
source = {{ kind = "response", pointer = "/findings" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/decision/scanned_bytes"
source = {{ kind = "response", pointer = "/scanned_bytes" }}
when_disposition = ["produced"]

[[capabilities.codec.response.meters]]
meter_id = "security.scanned_bytes"
unit = "bytes"
measurement_kind = "observed"
value_pointer = "/scanned_bytes"
"#
        ),
        FixtureKind::ContentInspect
        | FixtureKind::ContentInspectPartial
        | FixtureKind::ContentInspectWrongCoverage
        | FixtureKind::ContentInspectFailing
        | FixtureKind::ContentInspectLeaking => format!(
            r#"
[[capabilities]]
capability = "security.content.inspect/v1"
authority = "observe"
scopes = ["tool_call"]
input_contract = {{ schema = "security.content.inspect.input/v1", resource = "input.schema.json", sha256 = "{CONTENT_INPUT_SHA256}" }}
output_contract = {{ schema = "security.content.inspect.output/v1", resource = "output.schema.json", sha256 = "{CONTENT_OUTPUT_SHA256}" }}
native_input = {{ resource = "native.schema.json", sha256 = "{EMPTY_SCHEMA_SHA256}" }}
native_output = {{ resource = "native.schema.json", sha256 = "{EMPTY_SCHEMA_SHA256}" }}

[capabilities.codec]
kind = "json-map/v1"

[[capabilities.codec.request.fields]]
target = "/content"
source = {{ kind = "input", pointer = "/artifact/content" }}
on_missing = "reject"

[capabilities.codec.response.disposition]
source = "/disposition"
on_unknown = "fail"

[capabilities.codec.response.disposition.values]
completed = "produced"
error = "failed"

[[capabilities.codec.response.output_fields]]
target = "/inspection/verdict"
source = {{ kind = "response", pointer = "/verdict" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/inspection/findings"
source = {{ kind = "response", pointer = "/findings" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/inspection/scanned_bytes"
source = {{ kind = "response", pointer = "/scanned_bytes" }}
when_disposition = ["produced"]

[[capabilities.codec.response.output_fields]]
target = "/inspection/truncated"
source = {{ kind = "response", pointer = "/truncated" }}
when_disposition = ["produced"]

[[capabilities.codec.response.meters]]
meter_id = "security.scanned_bytes"
unit = "bytes"
measurement_kind = "observed"
value_pointer = "/scanned_bytes"
"#
        ),
    }
}
