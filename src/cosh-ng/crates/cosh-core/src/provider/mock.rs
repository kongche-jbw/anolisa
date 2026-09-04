use async_trait::async_trait;
use futures::stream;

use super::{
    ContentGenerator, GenerateConfig, GenerateEvent, GenerateStream, Message, ToolDeclaration,
};

pub struct MockProvider {
    pub responses: Vec<Vec<GenerateEvent>>,
    call_index: std::sync::atomic::AtomicUsize,
    echo_history: bool,
    repeat_text: Option<String>,
    checkpoint_roundtrip: bool,
}

impl MockProvider {
    pub fn new(responses: Vec<Vec<GenerateEvent>>) -> Self {
        Self {
            responses,
            call_index: std::sync::atomic::AtomicUsize::new(0),
            echo_history: false,
            repeat_text: None,
            checkpoint_roundtrip: false,
        }
    }

    pub fn text_only(text: &str) -> Self {
        Self::new(vec![vec![
            GenerateEvent::TextDelta(text.to_string()),
            GenerateEvent::MessageEnd,
        ]])
    }

    /// Returns the same short text for every request, without exhausting.
    ///
    /// Compaction tests need one provider that can serve both Agent turns
    /// and summarizer calls deterministically.
    pub fn repeat_text(text: &str) -> Self {
        Self {
            responses: Vec::new(),
            call_index: std::sync::atomic::AtomicUsize::new(0),
            echo_history: false,
            repeat_text: Some(text.to_string()),
            checkpoint_roundtrip: false,
        }
    }

    pub fn with_tool_call(tool_name: &str, tool_id: &str, arguments: &str) -> Self {
        Self::new(vec![vec![
            GenerateEvent::TextDelta("Let me help.".to_string()),
            GenerateEvent::ToolCallStart {
                index: 0,
                id: tool_id.to_string(),
                name: tool_name.to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: arguments.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ]])
    }

    pub fn history_echo() -> Self {
        Self {
            responses: Vec::new(),
            call_index: std::sync::atomic::AtomicUsize::new(0),
            echo_history: true,
            repeat_text: None,
            checkpoint_roundtrip: false,
        }
    }

    /// Emits one hosted checkpoint call, then echoes its settled tool result.
    pub fn workspace_checkpoint_roundtrip() -> Self {
        Self {
            responses: Vec::new(),
            call_index: std::sync::atomic::AtomicUsize::new(0),
            echo_history: false,
            repeat_text: None,
            checkpoint_roundtrip: true,
        }
    }

    pub fn partial_error() -> Self {
        Self::new(vec![vec![
            GenerateEvent::TextDelta("partial response".to_string()),
            GenerateEvent::Error("recoverable mock provider error".to_string()),
        ]])
    }
}

#[async_trait]
impl ContentGenerator for MockProvider {
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolDeclaration],
        _config: &GenerateConfig,
    ) -> Result<GenerateStream, String> {
        if self.checkpoint_roundtrip {
            let call_index = self
                .call_index
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if call_index == 0 {
                let names = tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>();
                if names != ["ask_user_question", "workspace_checkpoint_create"] {
                    return Err(format!(
                        "checkpoint mock received unexpected tool inventory: {names:?}"
                    ));
                }
                return Ok(Box::pin(stream::iter(vec![
                    GenerateEvent::ToolCallStart {
                        index: 0,
                        id: "checkpoint-call".to_string(),
                        name: "workspace_checkpoint_create".to_string(),
                    },
                    GenerateEvent::ToolCallDelta {
                        index: 0,
                        arguments_delta: "{}".to_string(),
                    },
                    GenerateEvent::ToolCallEnd { index: 0 },
                    GenerateEvent::MessageEnd,
                ])));
            }
            if call_index == 1 {
                let settled = messages.iter().rev().find(|message| {
                    message.role == "tool"
                        && message.tool_call_id.as_deref() == Some("checkpoint-call")
                });
                let Some(settled) = settled else {
                    return Err(
                        "checkpoint mock did not receive the settled tool result".to_string()
                    );
                };
                return Ok(Box::pin(stream::iter(vec![
                    GenerateEvent::TextDelta(format!(
                        "gateway checkpoint result: {}",
                        settled.content.as_text()
                    )),
                    GenerateEvent::MessageEnd,
                ])));
            }
            return Err("checkpoint mock received an unexpected provider turn".to_string());
        }
        if let Some(text) = &self.repeat_text {
            return Ok(Box::pin(stream::iter(vec![
                GenerateEvent::TextDelta(text.clone()),
                GenerateEvent::MessageEnd,
            ])));
        }
        if self.echo_history {
            let history = messages
                .iter()
                .filter(|message| message.role == "user")
                .map(|message| message.content.as_text())
                .collect::<Vec<_>>()
                .join(" | ");
            return Ok(Box::pin(stream::iter(vec![
                GenerateEvent::TextDelta(format!("mock history: {history}")),
                GenerateEvent::MessageEnd,
            ])));
        }
        let idx = self
            .call_index
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let events =
            self.responses.get(idx).cloned().unwrap_or_else(|| {
                vec![GenerateEvent::Error("no more mock responses".to_string())]
            });
        Ok(Box::pin(stream::iter(events)))
    }

    fn cancel(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn mock_provider_text_only() {
        let provider = MockProvider::text_only("Hello!");
        let stream = provider
            .generate(&[], &[], &GenerateConfig::default())
            .await
            .unwrap();
        let events: Vec<_> = stream.collect().await;
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], GenerateEvent::TextDelta(t) if t == "Hello!"));
        assert!(matches!(&events[1], GenerateEvent::MessageEnd));
    }

    #[tokio::test]
    async fn mock_provider_with_tool_call() {
        let provider = MockProvider::with_tool_call("shell", "call-1", r#"{"command":"ls"}"#);
        let stream = provider
            .generate(&[], &[], &GenerateConfig::default())
            .await
            .unwrap();
        let events: Vec<_> = stream.collect().await;
        assert_eq!(events.len(), 5);
        assert!(matches!(&events[1], GenerateEvent::ToolCallStart { name, .. } if name == "shell"));
    }

    #[tokio::test]
    async fn mock_provider_multi_turn() {
        let provider = MockProvider::new(vec![
            vec![
                GenerateEvent::TextDelta("first".to_string()),
                GenerateEvent::MessageEnd,
            ],
            vec![
                GenerateEvent::TextDelta("second".to_string()),
                GenerateEvent::MessageEnd,
            ],
        ]);
        let s1 = provider
            .generate(&[], &[], &GenerateConfig::default())
            .await
            .unwrap();
        let e1: Vec<_> = s1.collect().await;
        assert!(matches!(&e1[0], GenerateEvent::TextDelta(t) if t == "first"));

        let s2 = provider
            .generate(&[], &[], &GenerateConfig::default())
            .await
            .unwrap();
        let e2: Vec<_> = s2.collect().await;
        assert!(matches!(&e2[0], GenerateEvent::TextDelta(t) if t == "second"));
    }
}
