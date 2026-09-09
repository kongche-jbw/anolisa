//! Synthetic protocol peers exercise subprocess exchange, not live Tokenless acceptance.

use aw_contracts::{canonical, Registry};
use aw_core::ports::ProviderHost;
use aw_tokenless_host::{Config, Limits, TokenlessHost};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const PREFIX: &str = "import sys,json,os\nr=json.load(sys.stdin)\nassert r['protocol_version']==2 and r['input']['content']=='alpha alpha alpha\\n'\nassert r['input']['capabilities']=={'replace_output':True,'recovery':{'kind':'none'},'replace_with_text':False}\nassert os.environ['TOKENLESS_STATS_ENABLED']=='0'\nassert os.environ['TOKENLESS_SLS_ENABLED']=='0'\nx={'protocol_version':2,'operation':'post_tool','attribution':r['attribution'],'result':{'output':'alpha','disposition':'applied','applied_operations':['json_cleanup'],'recoverability':'unrecoverable','before_tokens':5,'after_tokens':2,'stash_keys':[],'tokenizer_id':'fixture'}}\ny=x['result']\n";

fn host(suffix: &str) -> TokenlessHost {
    TokenlessHost::new(Config {
        provider_id: "tokenless-fixture".into(),
        provider_version: "fixture-1".into(),
        program: PathBuf::from("/usr/bin/python3"),
        args: vec!["-c".into(), format!("{PREFIX}{suffix}")],
        environment: BTreeMap::new(),
        limits: Limits {
            timeout_ms: 1000,
            input_bytes: 8192,
            output_bytes: 8192,
            stderr_bytes: 1024,
        },
    })
    .unwrap()
}
fn invocation(host: &TokenlessHost) -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/contracts.json")).unwrap();
    let mut call = fixture["capability-invocation-v1"].clone();
    let desc = host.registered_descriptor();
    for key in ["provider_id", "provider_version", "manifest_digest"] {
        call[key] = desc[key].clone();
    }
    for key in ["capability", "input_schema", "output_schema"] {
        call[key] = desc["capabilities"][0][key].clone();
    }
    call["input"] = fixture["context-projection-prepare-input-v2"].clone();
    call["input"]["artifact"]["tool_name"] = json!("Bash");
    call["input"]["artifact"]["content"] = json!("alpha alpha alpha\n");
    call["input"]["artifact"]["digest"] = json!(canonical::digest(b"alpha alpha alpha\n"));
    call["input"]["constraints"] =
        json!({"allow_text_reencoding":false,"accepted_reversibility":["unrecoverable"]});
    call["input_digest"] = json!(canonical::document_digest(&call["input"]).unwrap());
    call["budget"] = json!({"input_bytes":8192,"output_bytes":8192,"wall_time_ms":2000});
    call["deadline_at_ms"] = json!(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 5000
    );
    call
}
fn invoke(suffix: &str) -> aw_core::ports::ProviderResult {
    let mut host = host(suffix);
    let call = invocation(&host);
    host.invoke(&call).unwrap()
}
#[test]
fn candidate_binds_source_receipt_and_observed_bytes() {
    let mut host = host("print(json.dumps(x))");
    let call = invocation(&host);
    let result = host.invoke(&call).unwrap();
    assert_eq!(
        result.receipt["disposition"], "produced",
        "{}",
        result.receipt
    );
    assert_eq!(result.receipt["input_digest"], call["input_digest"]);
    assert_eq!(result.receipt["scope"], call["scope"]);
    let output = result.output.unwrap();
    assert_eq!(
        output["candidate"]["source_digest"],
        call["input"]["artifact"]["digest"]
    );
    assert_eq!(output["candidate"]["recovery"], json!({"mode":"none"}));
    assert_eq!(result.receipt["meters"][0]["value"], 18);
    assert_eq!(result.receipt["meters"][1]["value"], 5);
    Registry::new()
        .unwrap()
        .validate("provider-receipt-v1", &result.receipt)
        .unwrap();
}
#[test]
fn no_savings_and_dry_run_never_produce_candidate_or_savings() {
    for suffix in [
        "y.update(disposition='no_savings',applied_operations=[],recoverability='lossless');print(json.dumps(x))",
        "y.update(disposition='dry_run');print(json.dumps(x))",
        "y['output']=r['input']['content'];print(json.dumps(x))",
    ] {
        let result=invoke(suffix); assert_eq!(result.receipt["disposition"],"bypassed","{}",result.receipt);
        assert!(result.output.is_none()); assert_eq!(result.receipt["meters"],json!([]));
    }
}

#[test]
fn preserved_native_disposition_is_bound_to_receipt() {
    let mut host = host("y.update(disposition='no_savings');print(json.dumps(x))");
    let result = host.invoke(&invocation(&host)).unwrap();
    let mapping = host.native_mapping().unwrap();
    assert_eq!(result.receipt["disposition"], "bypassed");
    assert_eq!(mapping["native_disposition"], "no_savings");
    assert_eq!(mapping["projection_result"], "native_preserved");
    assert_eq!(
        result.receipt["evidence"][0]["digest"],
        canonical::document_digest(mapping).unwrap()
    );
}
#[test]
fn native_failure_malformed_protocol_and_unavailable_recovery_are_visible() {
    for suffix in [
        "sys.exit(7)",
        "print('{}')",
        "x['protocol_version']=1;print(json.dumps(x))",
        "y.update(disposition='tool_error',applied_operations=[],recoverability='lossless',additional_context='fixture');print(json.dumps(x))",
        "y.update(recoverability='retrievable',stash_keys=['missing']);print(json.dumps(x))",
    ] {
        let result=invoke(suffix); assert_eq!(result.receipt["disposition"],"failed","{}",result.receipt);
        assert!(result.output.is_none()); assert_eq!(result.receipt["meters"],json!([]));
    }
}
#[test]
fn provider_timeout_is_bounded_and_retains_failure_receipt() {
    let result = invoke("import time;time.sleep(2)");
    assert_eq!(result.receipt["disposition"], "failed");
    assert_eq!(result.receipt["error_code"], "provider_timeout");
}
#[test]
fn changed_input_and_provider_identity_are_rejected() {
    let mut host = host("print(json.dumps(x))");
    let mut call = invocation(&host);
    call["input"]["artifact"]["content"] = json!("changed");
    assert_eq!(host.invoke(&call).unwrap().receipt["disposition"], "failed");
    call["provider_version"] = json!("other");
    assert!(host.invoke(&call).is_err());
}

#[test]
fn native_lossless_is_explicitly_downgraded_only_with_caller_consent() {
    let mut host = host("y['recoverability']='lossless';print(json.dumps(x))");
    let mut call = invocation(&host);
    let result = host.invoke(&call).unwrap();
    assert_eq!(result.receipt["disposition"], "produced");
    assert_eq!(
        result.output.unwrap()["candidate"]["reversibility"],
        "unrecoverable"
    );
    let mapping = host.native_mapping().unwrap();
    assert_eq!(mapping["native_claim"], "lossless");
    assert_eq!(
        result.receipt["evidence"][0]["digest"],
        canonical::document_digest(mapping).unwrap()
    );
    call["input"]["constraints"]["accepted_reversibility"] = json!(["lossless"]);
    call["input_digest"] = json!(canonical::document_digest(&call["input"]).unwrap());
    let rejected = host.invoke(&call).unwrap();
    assert_eq!(
        rejected.receipt["error_code"],
        "unsupported_recovery_requirement"
    );
    assert!(host.native_mapping().is_none());
}
