//! Shared native hook process and provider fixtures.

use aw_adapters::Host;
use aw_contracts::canonical;
use aw_hook_cli::process_identity;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

pub struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!(
                "hook-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub fn settings(directory: &Directory, fail: bool) -> Value {
    let f: Value =
        serde_json::from_slice(include_bytes!("../../../../tests/fixtures/contracts.json"))
            .unwrap();
    let pid = std::process::id();
    let (_, start) = process_identity(pid).unwrap();
    let mut runtime = f["runtime-binding-v1"].clone();
    runtime["process_ref"] = json!(format!("pid:{pid}@{start}"));
    runtime["observation_source"] = json!("owned_child");
    let script = format!(
        r#"import sys,json,pathlib
if sys.argv[-1]=='--version':
 print('agent-sec-cli 0.12.0');sys.exit(0)
assert sys.argv[1:]==['scan-pii','--stdin','--format','json','--source','tool_output']
content=sys.stdin.buffer.read()
p=pathlib.Path('received');p.write_bytes(p.read_bytes()+content if p.exists() else content)
if {fail}:
 sys.stderr.write('private scanner failure');sys.exit(5)
print(json.dumps({{'ok':True,'verdict':'pass','elapsed_ms':0,'findings':[], 'summary':{{'total':0,'by_type':{{}},'by_category':{{}},'by_severity':{{}},'source':'tool_output','bytes_scanned':len(content),'truncated':False,'custom_rules':{{'status':'absent','rule_count':0,'runtime_error_count':0,'budget_exhausted':False,'truncated':False}}}}}}))
"#,
        fail = if fail { "True" } else { "False" }
    );
    json!({"runtime":runtime,"scope":f["capability-plan-v1"]["scope"],"agent_pid":pid,"agent_start_ticks":start,
        "qoder_single_turn_id":"turn-1","journal":directory.0.join("journal"),"include_low_confidence":false,
        "provider":{"provider_id":"sec-test","provider_version":"0.12.0","program":"/usr/bin/python3",
            "program_sha256":canonical::digest(&fs::read("/usr/bin/python3").unwrap()),"cwd":directory.0,
            "args":["-c",script],"environment":{},"pins":[],
            "limits":{"timeout_ms":5000,"input_bytes":1048576,"output_bytes":131072,"stderr_bytes":16384}}})
}

pub fn payload(host: Host) -> Value {
    let mut p = json!({"hook_event_name":"PostToolUse","session_id":"session-1","tool_use_id":"tool-1",
        "tool_name":"Bash","tool_input":{"command":"printf example"},"tool_response":"备注🙂 exact\n\t",
        "保留字段":{"number":1.25}});
    if host == Host::Codex {
        p["turn_id"] = json!("turn-1");
    }
    p
}

// Existing inspection-only integration targets share the other helpers.
#[allow(dead_code)]
pub fn projection_config(directory: &Directory, failure: &str) -> (Value, Value, String) {
    let golden: Value = serde_json::from_slice(include_bytes!(
        "../../../../tests/fixtures/tokenless-native.json"
    ))
    .unwrap();
    let case = &golden["cases"][0];
    let mut source = case["request"]["input"]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut candidate = case["response"]["result"]["output"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut native_result = case["response"]["result"].clone();
    if failure == "large" {
        let original: Value = serde_json::from_str(&source).unwrap();
        let projected: Value = serde_json::from_str(&candidate).unwrap();
        source = serde_json::to_string(&vec![original; 128]).unwrap();
        candidate = serde_json::to_string(&vec![projected; 128]).unwrap();
        assert!(source.is_ascii() && candidate.is_ascii());
        native_result["output"] = json!(candidate);
        native_result["before_tokens"] = json!(source.len().div_ceil(4));
        native_result["after_tokens"] = json!(candidate.len().div_ceil(4));
    }
    let mut hook = settings(directory, failure == "security");
    hook["provider"]["args"][1] = json!(hook["provider"]["args"][1].as_str().unwrap().replace(
        " print('agent-sec-cli",
        " pathlib.Path('security_probe').write_text('yes');print('agent-sec-cli"
    ));
    let script = format!(
        r#"import sys,json,pathlib,os,time
if sys.argv[-1]=='--version':
 pathlib.Path('tokenless_probe').write_text('yes');print('tokenless 0.8.1');sys.exit(0)
assert sys.argv[1:]==['compress']
request=json.load(sys.stdin)
pathlib.Path('tokenless_received').write_text(request['input']['content'])
if os.environ['MODE']=='cancel': time.sleep(10)
if os.environ['MODE']=='tokenless': sys.exit(7)
if os.environ['MODE']=='record_failure': pathlib.Path('records').chmod(0o755)
result=json.loads({native:?})
if os.environ['MODE']=='no_savings':
 result.update(output=request['input']['content'],disposition='no_savings',applied_operations=[],after_tokens=result['before_tokens'])
print(json.dumps({{'protocol_version':2,'operation':'post_tool','attribution':request['attribution'],'result':result}}))
"#,
        native = native_result.to_string()
    );
    if failure == "sensitive" {
        let fixture: Value = serde_json::from_slice(include_bytes!(
            "../../../aw-sec-core/tests/fixtures/native-pii.json"
        ))
        .unwrap();
        let native = &fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "credential")
            .unwrap()["output"];
        hook["provider"]["args"][1] = json!(format!(
            r#"import sys,json,pathlib
if sys.argv[-1]=='--version':
 print('agent-sec-cli 0.12.0');sys.exit(0)
content=sys.stdin.buffer.read();pathlib.Path('received').write_bytes(content)
result=json.loads({native:?});result['summary']['bytes_scanned']=len(content)
print(json.dumps(result))
"#,
            native = native.to_string()
        ));
    }
    let mut tokenless = hook["provider"].clone();
    tokenless["provider_id"] = json!("tokenless-test");
    tokenless["provider_version"] = json!("0.8.1");
    tokenless["args"] = json!(["-c", script]);
    tokenless["environment"] = json!({"MODE":failure,"TOKENLESS_STATS_ENABLED":"0","TOKENLESS_SLS_ENABLED":"0","TOKENLESS_COMPRESSION_ENABLED":"1"});
    let history = directory.0.join("history.jsonl");
    fs::write(&history, b"").unwrap();
    fs::set_permissions(&history, fs::Permissions::from_mode(0o600)).unwrap();
    let config = json!({"hook":hook,"tokenless":tokenless,"record_directory":directory.0.join("records"),
        "history_path":history,"history_profile":"qoder-cli-1.1.47/jsonl-v1","retention":"source_and_candidate",
        "max_observation_delay_ms":300000,"accepted_reversibility":["unrecoverable"],"allow_text_reencoding":false});
    let mut native = payload(Host::Qoder);
    native["cwd"] = json!(directory.0);
    native["transcript_path"] = json!(history);
    native["tool_response"] = json!({"kind":"completed","exitCode":0,"signal":null,"interrupted":false,"isImage":false,"noOutputExpected":false,"stderr":"","stdout":source});
    (config, native, candidate)
}
