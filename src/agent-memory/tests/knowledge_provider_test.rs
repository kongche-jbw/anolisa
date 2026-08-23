#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use agent_memory::knowledge::mant::{MantCliConfig, MantCliProvider};
use agent_memory::knowledge::{
    KnowledgeCapability, KnowledgeErrorCode, KnowledgeHealthStatus, KnowledgeItem,
    KnowledgeProvider, KnowledgeProviderDescriptor, KnowledgeQuery, KnowledgeResult,
    KnowledgeSelector,
};
use agent_memory::protocol::KnowledgeRef;
use serde_json::{Value, json};
use tempfile::TempDir;

static CLI_TEST_LOCK: Mutex<()> = Mutex::new(());

struct FakeProvider;

impl KnowledgeProvider for FakeProvider {
    fn descriptor(&self) -> KnowledgeResult<KnowledgeProviderDescriptor> {
        Ok(KnowledgeProviderDescriptor {
            provider_id: "fake".to_owned(),
            display_name: "Fake provider".to_owned(),
            version: Some("1".to_owned()),
            protocol: Some("fake/v1".to_owned()),
            capabilities: vec![KnowledgeCapability::Excerpt],
        })
    }

    fn query(&self, query: &KnowledgeQuery) -> KnowledgeResult<Vec<KnowledgeItem>> {
        query.validate()?;
        Ok(vec![KnowledgeItem {
            reference: KnowledgeRef {
                provider: "fake".to_owned(),
                document_id: query.document_id.clone(),
                selector: Some(query.reference_selector()),
                content_digest: Some("fixture:1".to_owned()),
                retrieved_at_ms: 1,
            },
            title: Some("Fixture".to_owned()),
            excerpt: "bounded excerpt".to_owned(),
            fingerprint: "fixture:1".to_owned(),
            score: Some(1.0),
        }])
    }
}

#[test]
fn provider_neutral_trait_reports_health_and_items() {
    let provider: &dyn KnowledgeProvider = &FakeProvider;
    let health = provider.health();
    assert_eq!(health.status, KnowledgeHealthStatus::Healthy);
    assert_eq!(
        health
            .descriptor
            .as_ref()
            .map(|value| value.provider_id.as_str()),
        Some("fake")
    );

    let items = provider
        .query(&excerpt_query(128))
        .expect("fake provider query should succeed");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].reference.provider, "fake");
    assert_eq!(items[0].excerpt, "bounded excerpt");
}

#[test]
fn unavailable_and_incompatible_cli_are_typed_degradation() {
    let _guard = CLI_TEST_LOCK.lock().expect("CLI test lock");
    let directory = TempDir::new().expect("temporary directory should be created");
    let unavailable =
        MantCliProvider::new(MantCliConfig::new(directory.path().join("missing-mant")));
    let unavailable_health = unavailable.health();
    assert_eq!(unavailable_health.status, KnowledgeHealthStatus::Degraded);
    assert_eq!(
        unavailable_health.error.map(|error| error.code),
        Some(KnowledgeErrorCode::Unavailable)
    );

    let incompatible_path = write_script(
        &directory,
        "incompatible-mant",
        r#"#!/bin/sh
printf '%s\n' '{"protocol":"mant.cli/v8","nativeApiVersion":"8","requestSchema":"mant.request/v8","excerptSchema":"mant.excerpt/v8","searchSchema":"mant.search/v8"}'
"#,
    );
    let incompatible = MantCliProvider::new(MantCliConfig::new(incompatible_path));
    let incompatible_health = incompatible.health();
    assert_eq!(incompatible_health.status, KnowledgeHealthStatus::Degraded);
    assert_eq!(
        incompatible_health.error.map(|error| error.code),
        Some(KnowledgeErrorCode::Incompatible)
    );
}

#[test]
fn mant_cli_uses_focused_json_and_never_returns_unfocused_manual() {
    let _guard = CLI_TEST_LOCK.lock().expect("CLI test lock");
    let directory = TempDir::new().expect("temporary directory should be created");
    let shell_marker = directory.path().join("shell-interpolation-marker");
    let document_id = format!("tar;$(touch {})", shell_marker.display());
    let pattern = "focused".to_owned();
    let response = search_response("tar manual", &pattern, "focused result from manual", 1, 5);
    let executable = write_search_script(&directory, "fake-mant", &response);
    let provider = MantCliProvider::new(MantCliConfig::new(executable));
    let query = KnowledgeQuery {
        document_id: document_id.clone(),
        selector: KnowledgeSelector::Search {
            pattern,
            context_lines: 1,
        },
        max_excerpt_bytes: 32,
        max_items: 5,
    };

    let items = provider
        .query(&query)
        .expect("fake ManT query should succeed");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].excerpt, "focused result from manual");
    assert!(!items[0].excerpt.contains("NEVER COPY"));
    assert!(items[0].excerpt.len() <= query.max_excerpt_bytes);
    assert_eq!(items[0].reference.document_id, document_id);
    assert_eq!(items[0].title.as_deref(), Some("tar manual"));
    assert!(items[0].fingerprint.starts_with("fnv1a64:"));
    assert_eq!(
        items[0].reference.content_digest.as_deref(),
        Some(items[0].fingerprint.as_str())
    );
    assert!(!shell_marker.exists());
}

#[test]
fn mant_search_zero_hits_is_a_successful_empty_result() {
    let _guard = CLI_TEST_LOCK.lock().expect("CLI test lock");
    let directory = TempDir::new().expect("temporary directory should be created");
    let response = empty_search_response("tar", "definitely-not-present", 0, 5);
    let executable = write_search_script(&directory, "empty-mant", &response);
    let provider = MantCliProvider::new(MantCliConfig::new(executable));
    let query = KnowledgeQuery {
        document_id: "tar".to_owned(),
        selector: KnowledgeSelector::Search {
            pattern: "definitely-not-present".to_owned(),
            context_lines: 0,
        },
        max_excerpt_bytes: 128,
        max_items: 5,
    };

    let items = provider
        .query(&query)
        .expect("a legal no-match search is not a provider failure");
    assert!(items.is_empty());
}

#[test]
fn mant_search_rejects_missing_required_fields_and_wrong_structures() {
    let _guard = CLI_TEST_LOCK.lock().expect("CLI test lock");
    let directory = TempDir::new().expect("temporary directory should be created");
    let query = KnowledgeQuery {
        document_id: "tar".to_owned(),
        selector: KnowledgeSelector::Search {
            pattern: "focused".to_owned(),
            context_lines: 1,
        },
        max_excerpt_bytes: 128,
        max_items: 5,
    };
    let mut missing_render = search_response("tar", "focused", "focused result", 1, 5);
    missing_render
        .as_object_mut()
        .expect("search response object")
        .remove("render");
    let mut wrong_matches = search_response("tar", "focused", "focused result", 1, 5);
    wrong_matches["matches"] = json!({"preview": "focused result"});

    for (name, response) in [
        ("missing-render-mant", missing_render),
        ("wrong-matches-mant", wrong_matches),
    ] {
        let executable = write_search_script(&directory, name, &response);
        let provider = MantCliProvider::new(MantCliConfig::new(executable));
        let error = provider
            .query(&query)
            .expect_err("malformed ManT wire must be rejected");
        assert_eq!(error.code, KnowledgeErrorCode::MalformedResponse);
    }
}

#[test]
fn mant_explain_accepts_typed_entry_below_an_outline_ancestor() {
    let _guard = CLI_TEST_LOCK.lock().expect("CLI test lock");
    let directory = TempDir::new().expect("temporary directory should be created");
    let response = json!({
        "schema": "mant.excerpt/v0.9",
        "label": "bash",
        "selections": [{
            "kind": "document-entry",
            "outline": {
                "ancestors": [{"path": "1", "id": "pipelines", "title": "PIPELINES"}],
                "node": {
                    "kind": "document-entry",
                    "path": "1/e1",
                    "id": "pipestatus",
                    "title": "PIPESTATUS",
                    "role": "environment-variable",
                    "case": "sensitive",
                    "names": ["PIPESTATUS"]
                }
            },
            "entry": {
                "terms": [[{"type": "code", "value": "PIPESTATUS"}]],
                "description": [{
                    "type": "paragraph",
                    "children": [{"type": "text", "value": "Pipeline exit statuses."}]
                }]
            }
        }]
    });
    let executable = write_response_script(&directory, "explain-mant", &response, "explain");
    let provider = MantCliProvider::new(MantCliConfig::new(executable));
    let items = provider
        .query(&KnowledgeQuery {
            document_id: "bash".to_owned(),
            selector: KnowledgeSelector::Explain {
                entry: "PIPESTATUS".to_owned(),
            },
            max_excerpt_bytes: 256,
            max_items: 1,
        })
        .expect("official nested entry response should be accepted");

    assert_eq!(items.len(), 1);
    assert!(items[0].excerpt.contains("PIPESTATUS"));
    assert!(items[0].excerpt.contains("Pipeline exit statuses."));
}

#[test]
fn request_writer_obeys_deadline_when_child_never_reads_stdin() {
    let _guard = CLI_TEST_LOCK.lock().expect("CLI test lock");
    let directory = TempDir::new().expect("temporary directory should be created");
    let executable = write_script(
        &directory,
        "stdin-stalled-mant",
        r#"#!/bin/sh
if [ "$1" = "--protocol-version" ] && [ "$2" = "--compact" ]; then
  printf '%s\n' '{"protocol":"mant.cli/v0.9","nativeApiVersion":"0.9.0","requestSchema":"mant.request/v0.9","excerptSchema":"mant.excerpt/v0.9","searchSchema":"mant.search/v0.9"}'
  exit 0
fi
if [ "$1" = "--request-json" ] && [ "$2" = "--format" ] && [ "$3" = "json" ] && [ "$4" = "--compact" ]; then
  sleep 30 <&0 &
  exit 0
fi
exit 2
"#,
    );
    let mut config = MantCliConfig::new(executable);
    config.timeout = Duration::from_millis(250);
    let provider = MantCliProvider::new(config);
    let document_id = "\u{1}".repeat(4096);
    let pattern = "\u{1}".repeat(4089);
    let query = KnowledgeQuery {
        document_id: document_id.clone(),
        selector: KnowledgeSelector::Search {
            pattern: pattern.clone(),
            context_lines: 100,
        },
        max_excerpt_bytes: 64 * 1024,
        max_items: 64,
    };
    query.validate().expect("boundary-sized query is valid");
    let wire = serde_json::to_vec(&json!({
        "schema": "mant.request/v0.9",
        "input": {"kind": "document", "selector": document_id},
        "view": {
            "kind": "search",
            "pattern": pattern,
            "syntax": "literal",
            "case": "insensitive",
            "scope": "visible",
            "word": false,
            "contextLines": 100,
            "limit": 64,
            "offset": 0
        }
    }))
    .expect("request fixture serialization");
    assert!(wire.len() > 48 * 1024);
    assert!(wire.len() < 64 * 1024);

    let started = Instant::now();
    let error = provider
        .query(&query)
        .expect_err("a child that never reads stdin must time out");
    assert_eq!(error.code, KnowledgeErrorCode::Timeout);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn cli_timeout_kills_the_child_and_returns_promptly() {
    let _guard = CLI_TEST_LOCK.lock().expect("CLI test lock");
    let directory = TempDir::new().expect("temporary directory should be created");
    let executable = write_script(
        &directory,
        "stuck-mant",
        r#"#!/bin/sh
sleep 30 &
wait
"#,
    );
    let mut config = MantCliConfig::new(executable);
    config.timeout = Duration::from_millis(250);
    let provider = MantCliProvider::new(config);

    let started = Instant::now();
    let health = provider.health();
    assert_eq!(health.status, KnowledgeHealthStatus::Degraded);
    assert_eq!(
        health.error.map(|error| error.code),
        Some(KnowledgeErrorCode::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn cli_output_is_drained_but_not_retained_past_the_bound() {
    let _guard = CLI_TEST_LOCK.lock().expect("CLI test lock");
    let directory = TempDir::new().expect("temporary directory should be created");
    let executable = write_script(
        &directory,
        "noisy-mant",
        r#"#!/bin/sh
i=0
while [ "$i" -lt 4096 ]; do
  printf x
  i=$((i + 1))
done
"#,
    );
    let mut config = MantCliConfig::new(executable);
    config.max_stdout_bytes = 1024;
    let provider = MantCliProvider::new(config);
    let health = provider.health();
    assert_eq!(health.status, KnowledgeHealthStatus::Degraded);
    assert_eq!(
        health.error.map(|error| error.code),
        Some(KnowledgeErrorCode::ResourceExhausted)
    );
}

#[test]
fn query_rejects_unbounded_selector_aggregates() {
    let mut query = excerpt_query(128);
    query.selector = KnowledgeSelector::Excerpt {
        selectors: vec!["x".repeat(4096), "y".to_owned()],
    };
    let error = query.validate().expect_err("aggregate must remain bounded");
    assert_eq!(error.code, KnowledgeErrorCode::ResourceExhausted);
}

fn excerpt_query(max_excerpt_bytes: usize) -> KnowledgeQuery {
    KnowledgeQuery {
        document_id: "fixture-document".to_owned(),
        selector: KnowledgeSelector::Excerpt {
            selectors: vec!["focused-section".to_owned()],
        },
        max_excerpt_bytes,
        max_items: 1,
    }
}

fn write_script(directory: &TempDir, name: &str, contents: &str) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, contents).expect("fake executable should be written");
    let mut permissions = fs::metadata(&path)
        .expect("fake executable metadata should be readable")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).expect("fake executable should be executable");
    path
}

fn write_search_script(directory: &TempDir, name: &str, response: &Value) -> PathBuf {
    write_response_script(directory, name, response, "search")
}

fn write_response_script(
    directory: &TempDir,
    name: &str,
    response: &Value,
    expected_kind: &str,
) -> PathBuf {
    let response = serde_json::to_string(response).expect("search response serialization");
    let response = shell_single_quote(&response);
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--protocol-version" ] && [ "$2" = "--compact" ]; then
  printf '%s\n' '{{"protocol":"mant.cli/v0.9","nativeApiVersion":"0.9.0","requestSchema":"mant.request/v0.9","excerptSchema":"mant.excerpt/v0.9","searchSchema":"mant.search/v0.9"}}'
  exit 0
fi
if [ "$1" != "--request-json" ] || [ "$2" != "--format" ] || [ "$3" != "json" ] || [ "$4" != "--compact" ]; then
  exit 2
fi
request=$(/bin/cat)
case "$request" in
  *'"kind":"{expected_kind}"'*) ;;
  *) exit 2 ;;
esac
printf '%s\n' {response}
"#
    );
    write_script(directory, name, &script)
}

fn search_response(
    label: &str,
    pattern: &str,
    preview: &str,
    context_lines: u8,
    limit: u16,
) -> Value {
    let end_byte = u64::try_from(pattern.len()).expect("fixture byte length");
    let end_column = u64::try_from(pattern.chars().count() + 1).expect("fixture column");
    json!({
        "schema": "mant.search/v0.9",
        "label": label,
        "query": search_query(pattern, context_lines, limit),
        "render": search_render(),
        "total": 1,
        "returned": 1,
        "offset": 0,
        "truncated": false,
        "matches": [{
            "ordinal": 1,
            "outline": {
                "ancestors": [{
                    "path": "0",
                    "id": "document-root",
                    "title": "Document root"
                }],
                "node": {
                    "kind": "document-section",
                    "path": "1",
                    "id": "focused-section",
                    "title": "Focused section"
                }
            },
            "occurrences": [{
                "matchedText": pattern,
                "markdown": {
                    "startByte": 0,
                    "endByte": end_byte,
                    "startLine": 1,
                    "startColumn": 1,
                    "endLine": 1,
                    "endColumn": end_column
                },
                "lineRanges": [{"line": 1, "startByte": 0, "endByte": end_byte}]
            }],
            "occurrenceCount": 1,
            "occurrencesTruncated": false,
            "preview": preview,
            "context": [{"line": 1, "text": preview, "matched": true}]
        }]
    })
}

fn empty_search_response(label: &str, pattern: &str, context_lines: u8, limit: u16) -> Value {
    json!({
        "schema": "mant.search/v0.9",
        "label": label,
        "query": search_query(pattern, context_lines, limit),
        "render": search_render(),
        "total": 0,
        "returned": 0,
        "offset": 0,
        "truncated": false,
        "matches": []
    })
}

fn search_query(pattern: &str, context_lines: u8, limit: u16) -> Value {
    json!({
        "pattern": pattern,
        "syntax": "literal",
        "case": "insensitive",
        "scope": "visible",
        "word": false,
        "contextLines": context_lines,
        "limit": limit,
        "offset": 0
    })
}

fn search_render() -> Value {
    json!({
        "schema": "mant.markdown/v1",
        "format": "markdown",
        "scope": "full",
        "lineBase": 1,
        "columnBase": 1,
        "lineCount": 1
    })
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
