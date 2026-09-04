// Contract tests for protocol v1. The two *_roadmap_example tests pin the
// §4.1 JSON examples from the evolution roadmap draft. That document has not
// landed in-repo yet, so until it does the JSON here is the authoritative
// wire contract; once it lands, these tests become the drift guard between
// the document and the types.

fn applied_response(reversibility: Reversibility, stash_keys: Vec<String>) -> CompressionResponse {
    CompressionResponse {
        protocol_version: PROTOCOL_VERSION,
        output: "compressed".into(),
        disposition: Disposition::Applied,
        output_media_type: "text/plain".into(),
        content_type: Some("json".into()),
        compressor_chain: vec!["json-cleanup".into()],
        reversibility,
        before_tokens: 10,
        after_tokens: 4,
        stash_keys,
        tokenizer_id: TOKENIZER_ID.into(),
        diagnostic: None,
    }
}

#[test]
fn request_roadmap_example_parses() {
    let json = r#"{
  "protocol_version": 1,
  "content": "...",
  "agent_id": "claude-code",
  "session_id": "...",
  "tool_use_id": "...",
  "tool_name": "Bash",
  "seam": "post_tool",
  "capabilities": {
    "replace_output": true,
    "publish_retrieve_tool": true
  }
}"#;
    let req = CompressionRequest::from_json(json).expect("roadmap request example must parse");
    assert_eq!(req.protocol_version, PROTOCOL_VERSION);
    assert_eq!(req.agent_id, "claude-code");
    assert_eq!(req.tool_name.as_deref(), Some("Bash"));
    assert_eq!(req.seam, Seam::PostTool);
    assert!(req.capabilities.replace_output);
    assert!(req.capabilities.publish_retrieve_tool);
}

#[test]
fn response_roadmap_example_parses() {
    let json = r#"{
  "protocol_version": 1,
  "output": "...",
  "disposition": "applied",
  "content_type": "build_log",
  "compressor_chain": ["terminal-cleanup", "build-log"],
  "reversibility": "retrievable",
  "before_tokens": 1200,
  "after_tokens": 340,
  "stash_keys": ["0123456789abcdef01234567"]
}"#;
    let resp = CompressionResponse::from_json(json).expect("roadmap response example must parse");
    assert!(resp.is_applied());
    assert_eq!(resp.content_type.as_deref(), Some("build_log"));
    assert_eq!(resp.compressor_chain, ["terminal-cleanup", "build-log"]);
    assert_eq!(resp.reversibility, Reversibility::Retrievable);
    assert_eq!((resp.before_tokens, resp.after_tokens), (1200, 340));
    assert_eq!(resp.stash_keys, ["0123456789abcdef01234567"]);
    assert_eq!(resp.output_media_type, DEFAULT_OUTPUT_MEDIA_TYPE);
    // The example predates the field; absence reads as the heuristic
    // estimator, the only counter that ever shipped before the field.
    assert_eq!(resp.tokenizer_id, TOKENIZER_ID);
}

#[test]
fn request_round_trips() {
    let mut req = CompressionRequest::new("hello world", "codex", Seam::PostTool);
    req.input_media_type = Some("application/json".into());
    req.session_id = Some("s-1".into());
    req.tool_use_id = Some("tu-1".into());
    req.tool_name = Some("Bash".into());
    req.capabilities.replace_output = true;
    let parsed = CompressionRequest::from_json(&req.to_json().unwrap()).unwrap();
    assert_eq!(parsed, req);
}

#[test]
fn response_round_trips() {
    let req = CompressionRequest::new("payload", "claude-code", Seam::PreTool);
    let resp = CompressionResponse::passthrough(&req, 42);
    let parsed = CompressionResponse::from_json(&resp.to_json().unwrap()).unwrap();
    assert_eq!(parsed, resp);
}

#[test]
fn unknown_fields_are_ignored() {
    let json = r#"{
  "protocol_version": 1,
  "content": "x",
  "agent_id": "a",
  "seam": "post_tool",
  "future_optional_field": {"nested": [1, 2, 3]}
}"#;
    let req = CompressionRequest::from_json(json).expect("unknown optional fields are ignored");
    assert_eq!(req.content, "x");
}

#[test]
fn missing_optionals_take_defaults() {
    let json = r#"{"protocol_version":1,"content":"x","agent_id":"a","seam":"before_model"}"#;
    let req = CompressionRequest::from_json(json).unwrap();
    assert_eq!(req.session_id, None);
    assert_eq!(req.tool_use_id, None);
    assert_eq!(req.tool_name, None);
    assert_eq!(req.input_media_type, None);
    assert!(!req.capabilities.replace_output);
    assert!(!req.capabilities.publish_retrieve_tool);
    assert!(!req.capabilities.replace_with_text);
}

#[test]
fn replace_with_text_defaults_false_and_parses_when_declared() {
    // Requests from adapters predating the field must keep the conservative
    // structured-slot semantics.
    let json = r#"{"protocol_version":1,"content":"x","agent_id":"a","seam":"post_tool","capabilities":{"replace_output":true}}"#;
    let req = CompressionRequest::from_json(json).unwrap();
    assert!(!req.capabilities.replace_with_text);

    let json = r#"{"protocol_version":1,"content":"x","agent_id":"a","seam":"post_tool","capabilities":{"replace_output":true,"replace_with_text":true}}"#;
    let req = CompressionRequest::from_json(json).unwrap();
    assert!(req.capabilities.replace_with_text);
}

#[test]
fn unsupported_version_beats_shape_errors() {
    // A v2 payload with a shape v1 cannot parse must still be reported as a
    // version problem, not a malformed payload.
    let json = r#"{"protocol_version":2,"body":{"parts":["..."]}}"#;
    match CompressionRequest::from_json(json) {
        Err(ProtocolError::UnsupportedVersion { found: 2 }) => {}
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
    match CompressionResponse::from_json(json) {
        Err(ProtocolError::UnsupportedVersion { found: 2 }) => {}
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[test]
fn malformed_payload_is_reported() {
    let err = CompressionRequest::from_json("not json").unwrap_err();
    assert!(matches!(err, ProtocolError::Malformed(_)));
    // The structured serde error stays reachable through the source chain.
    assert!(std::error::Error::source(&err).is_some());
    // Valid version, wrong shape for the rest.
    assert!(matches!(
        CompressionRequest::from_json(r#"{"protocol_version":1,"content":7}"#),
        Err(ProtocolError::Malformed(_))
    ));
}

#[test]
fn direct_deserialization_cannot_bypass_the_version_gate() {
    // A v2 payload whose remaining fields happen to fit the v1 shape must
    // fail even through plain serde, not only through from_json.
    let json = r#"{"protocol_version":2,"content":"x","agent_id":"a","seam":"post_tool"}"#;
    let err = serde_json::from_str::<CompressionRequest>(json).unwrap_err();
    assert!(err.to_string().contains("unsupported protocol_version 2"));

    let json = r#"{"protocol_version":2,"output":"o","disposition":"applied","reversibility":"lossless","before_tokens":1,"after_tokens":1}"#;
    let err = serde_json::from_str::<CompressionResponse>(json).unwrap_err();
    assert!(err.to_string().contains("unsupported protocol_version 2"));
}

#[test]
fn passthrough_is_canonical() {
    let req = CompressionRequest::new("original", "qoder-cli", Seam::PostTool);
    let resp = CompressionResponse::passthrough(&req, 9);
    assert_eq!(resp.output, "original");
    assert_eq!(resp.disposition, Disposition::Passthrough);
    assert_eq!(resp.reversibility, Reversibility::Lossless);
    assert_eq!((resp.before_tokens, resp.after_tokens), (9, 9));
    assert!(resp.compressor_chain.is_empty());
    assert!(resp.stash_keys.is_empty());
    assert_eq!(resp.tokenizer_id, TOKENIZER_ID);
    assert!(!resp.is_applied());
}

#[test]
fn wire_format_is_stable() {
    // Locks field names and enum wire values. A failure here is a protocol
    // change and requires either a compatible optional field or a new
    // protocol_version — never a silent rename.
    let mut req = CompressionRequest::new("c", "a", Seam::PostTool);
    req.capabilities.replace_output = true;
    assert_eq!(
        req.to_json().unwrap(),
        r#"{"protocol_version":1,"content":"c","agent_id":"a","seam":"post_tool","capabilities":{"replace_output":true,"publish_retrieve_tool":false,"replace_with_text":false}}"#
    );

    // A declared origin adds one field; an unspecified one stays off the
    // wire entirely, which is what keeps unmigrated adapters byte-identical.
    req.content_origin = ContentOrigin::FileContent;
    assert_eq!(
        req.to_json().unwrap(),
        r#"{"protocol_version":1,"content":"c","agent_id":"a","seam":"post_tool","content_origin":"file_content","capabilities":{"replace_output":true,"publish_retrieve_tool":false,"replace_with_text":false}}"#
    );

    let mut resp = applied_response(Reversibility::Retrievable, vec!["k".into()]);
    resp.output = "o".into();
    resp.content_type = Some("search_results".into());
    resp.compressor_chain = vec!["search".into()];
    assert_eq!(
        resp.to_json().unwrap(),
        r#"{"protocol_version":1,"output":"o","output_media_type":"text/plain","disposition":"applied","content_type":"search_results","compressor_chain":["search"],"reversibility":"retrievable","before_tokens":10,"after_tokens":4,"stash_keys":["k"],"tokenizer_id":"heuristic-v1"}"#
    );

    let mut error = CompressionResponse::passthrough(
        &CompressionRequest::new("o", "a", Seam::PostTool),
        1,
    );
    error.disposition = Disposition::Error;
    error.diagnostic = Some("d".into());
    assert_eq!(
        error.to_json().unwrap(),
        r#"{"protocol_version":1,"output":"o","output_media_type":"text/plain","disposition":"error","compressor_chain":[],"reversibility":"lossless","before_tokens":1,"after_tokens":1,"stash_keys":[],"tokenizer_id":"heuristic-v1","diagnostic":"d"}"#
    );
}

#[test]
fn response_state_validation_rejects_false_recovery_claims() {
    let mut response = applied_response(Reversibility::Retrievable, Vec::new());
    assert_eq!(
        response.validate(),
        Err(ResponseStateError::RetrievableWithoutStashKey)
    );
    assert!(matches!(
        response.to_json(),
        Err(ProtocolError::InvalidResponseState(
            ResponseStateError::RetrievableWithoutStashKey
        ))
    ));

    response.reversibility = Reversibility::Lossless;
    response.stash_keys.push("unexpected".into());
    assert_eq!(
        response.validate(),
        Err(ResponseStateError::LosslessWithStashKeys)
    );

    // An overall unrecoverable candidate may still expose keys for the
    // independently recoverable subset without claiming full recovery.
    response.reversibility = Reversibility::Unrecoverable;
    assert_eq!(response.validate(), Ok(()));
}

#[test]
fn response_state_validation_rejects_disposition_contradictions() {
    let mut response = applied_response(Reversibility::Unrecoverable, Vec::new());
    response.compressor_chain.clear();
    assert!(matches!(
        response.validate(),
        Err(ResponseStateError::MissingCompressorChain {
            disposition: Disposition::Applied
        })
    ));

    response = CompressionResponse::passthrough(
        &CompressionRequest::new("source", "a", Seam::PostTool),
        2,
    );
    response.compressor_chain.push("impossible".into());
    assert!(matches!(
        response.validate(),
        Err(ResponseStateError::UnexpectedCompressorChain {
            disposition: Disposition::Passthrough
        })
    ));

    response.compressor_chain.clear();
    response.diagnostic = Some("only errors may diagnose".into());
    assert!(matches!(
        response.validate(),
        Err(ResponseStateError::DiagnosticOnNonError {
            disposition: Disposition::Passthrough
        })
    ));
}

#[test]
fn response_decode_enforces_state_validation() {
    let json = r#"{
      "protocol_version": 1,
      "output": "source",
      "disposition": "passthrough",
      "compressor_chain": ["json-cleanup"],
      "reversibility": "lossless",
      "before_tokens": 2,
      "after_tokens": 2,
      "stash_keys": [],
      "tokenizer_id": "heuristic-v1"
    }"#;
    assert!(matches!(
        CompressionResponse::from_json(json),
        Err(ProtocolError::InvalidResponseState(
            ResponseStateError::UnexpectedCompressorChain {
                disposition: Disposition::Passthrough
            }
        ))
    ));
}

#[test]
fn every_origin_round_trips_and_absence_reads_as_unspecified() {
    for origin in [
        ContentOrigin::Unspecified,
        ContentOrigin::CommandOutput,
        ContentOrigin::FileContent,
        ContentOrigin::ApiResponse,
    ] {
        let mut req = CompressionRequest::new("c", "a", Seam::PostTool);
        req.content_origin = origin;
        let parsed = CompressionRequest::from_json(&req.to_json().unwrap()).unwrap();
        assert_eq!(parsed.content_origin, origin);
        // `wire_str` is hand-written beside a serde rename: assert the two
        // agree, rather than that the enum equals itself.
        assert_eq!(
            serde_json::to_value(origin).unwrap(),
            serde_json::Value::String(origin.wire_str().to_owned())
        );
    }

    // The pre-migration payload: no field at all.
    let legacy = r#"{"protocol_version":1,"content":"c","agent_id":"a","seam":"post_tool"}"#;
    let parsed = CompressionRequest::from_json(legacy).unwrap();
    assert_eq!(parsed.content_origin, ContentOrigin::Unspecified);
    assert!(parsed.content_origin.is_unspecified());
}

#[test]
fn all_seams_and_dispositions_round_trip() {
    for (seam, wire) in [
        (Seam::BeforeModel, "\"before_model\""),
        (Seam::PreTool, "\"pre_tool\""),
        (Seam::PostTool, "\"post_tool\""),
        (Seam::Proxy, "\"proxy\""),
    ] {
        assert_eq!(serde_json::to_string(&seam).unwrap(), wire);
        assert_eq!(serde_json::from_str::<Seam>(wire).unwrap(), seam);
        assert_eq!(format!("\"{}\"", seam.wire_str()), wire);
    }
    for (disp, wire) in [
        (Disposition::Applied, "\"applied\""),
        (Disposition::DryRun, "\"dry_run\""),
        (Disposition::Passthrough, "\"passthrough\""),
        (Disposition::NoSavings, "\"no_savings\""),
        (
            Disposition::ReversibilityUnavailable,
            "\"reversibility_unavailable\"",
        ),
        (Disposition::Timeout, "\"timeout\""),
        (Disposition::Error, "\"error\""),
    ] {
        assert_eq!(serde_json::to_string(&disp).unwrap(), wire);
        assert_eq!(serde_json::from_str::<Disposition>(wire).unwrap(), disp);
        assert_eq!(format!("\"{}\"", disp.wire_str()), wire);
    }
    for (rev, wire) in [
        (Reversibility::Lossless, "\"lossless\""),
        (Reversibility::Retrievable, "\"retrievable\""),
        (Reversibility::Unrecoverable, "\"unrecoverable\""),
    ] {
        assert_eq!(serde_json::to_string(&rev).unwrap(), wire);
        assert_eq!(serde_json::from_str::<Reversibility>(wire).unwrap(), rev);
    }
}
