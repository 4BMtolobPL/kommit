use crate::LmStudio;
use crate::chat::request::ChatRequest;
use crate::chat::stream::response::StreamEvent;
use crate::{Error, Result};
use async_stream::stream;
use std::pin::Pin;
use tokio_stream::{Stream, StreamExt};
use tracing::{debug, error, info, instrument};

pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

#[cfg(feature = "stream")]
impl LmStudio {
    #[instrument(skip(self, request), fields(url = %self.url, endpoint = "/api/v1/chat", model = request.model))]
    pub async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        info!("Send a message to a model and receive a response. Supports MCP integration.");

        let url = self.endpoint("api/v1/chat")?;
        let request = request.with_stream(true);
        let res = self.client.post(url).json(&request).send().await?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(Error::Server { status, body });
        }

        let s = stream! {
            let mut decoder = SseDecoder::default();
            let mut stream = res.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        for event in decoder.feed(&chunk) {
                            yield event;
                        }
                        if decoder.is_done() {
                            return;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to read response");
                        yield Err(Error::Http(e));
                        return;
                    }
                }
            }

            for event in decoder.finish() {
                yield event;
            }
        };

        Ok(Box::pin(s))
    }
}

/// Turns the bytes of a server-sent event stream into events, however the bytes
/// are split into chunks.
#[derive(Default)]
struct SseDecoder {
    /// Bytes of a line that has not ended yet.
    buffer: Vec<u8>,
    /// Whether `data: [DONE]` was seen; everything after it is ignored.
    done: bool,
}

impl SseDecoder {
    /// Decodes every line `chunk` completes.
    fn feed(&mut self, chunk: &[u8]) -> Vec<Result<StreamEvent>> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while !self.done {
            let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') else {
                break;
            };
            let line: Vec<u8> = self.buffer.drain(..=pos).collect();
            self.decode_line(&line[..pos], &mut events);
        }
        events
    }

    /// Decodes the last line, which the stream may end without a newline.
    fn finish(&mut self) -> Vec<Result<StreamEvent>> {
        let line = std::mem::take(&mut self.buffer);
        let mut events = Vec::new();
        if !self.done {
            self.decode_line(&line, &mut events);
        }
        events
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn decode_line(&mut self, line: &[u8], events: &mut Vec<Result<StreamEvent>>) {
        let line = String::from_utf8_lossy(line);
        let line = line.trim();

        if let Some(ev) = line.strip_prefix("event:") {
            debug!(event = %ev.trim(), "SSE event type");
        } else if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data == "[DONE]" {
                self.done = true;
                return;
            }

            match serde_json::from_str::<StreamEvent>(data) {
                Ok(response) => events.push(Ok(response)),
                Err(e) => {
                    error!(error = %e, data = %data, "Failed to deserialize response");
                    events.push(Err(Error::Decode(e)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = r#"data: {"type":"message.start"}"#;

    /// Feeds `chunks` in order, then ends the stream.
    fn decode(chunks: &[&[u8]]) -> Vec<Result<StreamEvent>> {
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(decoder.feed(chunk));
        }
        events.extend(decoder.finish());
        events
    }

    fn delta(content: &str) -> String {
        format!(
            "event: message.delta\ndata: {}\n\n",
            serde_json::json!({ "type": "message.delta", "content": content })
        )
    }

    fn contents(events: Vec<Result<StreamEvent>>) -> Vec<String> {
        events
            .into_iter()
            .map(|event| match event {
                Ok(StreamEvent::MessageDelta { content }) => content,
                other => panic!("expected a message delta, got {other:?}"),
            })
            .collect()
    }

    /// Covers splits inside a line, between events, and inside a multi-byte character.
    #[test]
    fn every_split_point_gives_the_same_events() {
        let body = format!("{}{}", delta("one"), delta("한글"));
        for at in 0..=body.len() {
            let (first, second) = body.as_bytes().split_at(at);
            assert_eq!(
                contents(decode(&[first, second])),
                ["one", "한글"],
                "split at {at}"
            );
        }
    }

    #[test]
    fn crlf_line_endings_are_decoded() {
        let body = delta("crlf").replace('\n', "\r\n");
        assert_eq!(contents(decode(&[body.as_bytes()])), ["crlf"]);
    }

    #[test]
    fn done_ends_the_stream() {
        let body = format!("{}data: [DONE]\n\n{}", delta("before"), delta("after"));

        let mut decoder = SseDecoder::default();
        assert_eq!(contents(decoder.feed(body.as_bytes())), ["before"]);
        assert!(decoder.is_done());
        assert!(decoder.feed(delta("later").as_bytes()).is_empty());
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn invalid_data_is_an_error_and_decoding_continues() {
        let body = format!("data: {{not json\n\n{}", delta("next"));
        let events = decode(&[body.as_bytes()]);

        assert_eq!(events.len(), 2, "{events:?}");
        assert!(matches!(events[0], Err(Error::Decode(_))), "{events:?}");
        assert!(
            matches!(events[1], Ok(StreamEvent::MessageDelta { ref content }) if content == "next")
        );
    }

    #[test]
    fn a_last_line_without_a_newline_is_decoded_when_the_stream_ends() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.feed(START.as_bytes()).is_empty());

        let events = decoder.finish();
        assert!(
            matches!(events[..], [Ok(StreamEvent::MessageStart)]),
            "{events:?}"
        );
    }

    #[test]
    fn lines_other_than_data_make_no_events() {
        let body = "\n\nevent: message.start\n: a comment\nid: 1\n\n";
        assert!(decode(&[body.as_bytes()]).is_empty());
    }
}

#[cfg(feature = "stream")]
pub mod response {
    use crate::chat::response::{ChatResponse, Metadata, ProviderInfo};
    use serde::Deserialize;
    use std::collections::HashMap;

    #[derive(Deserialize, Debug)]
    #[serde(tag = "type")]
    pub enum StreamEvent {
        /// An event that is emitted at the start of a chat response stream.
        #[serde(rename = "chat.start")]
        ChatStart { model_instance_id: String },
        /// Signals the start of a model being loaded to fulfill the chat request. Will not be emitted if the requested model is already loaded.
        #[serde(rename = "model_load.start")]
        ModelLoadStart { model_instance_id: String },
        #[serde(rename = "model_load.progress")]
        ModelLoadProgress {
            model_instance_id: String,
            progress: f64,
        },
        #[serde(rename = "model_load.end")]
        ModelLoadEnd {
            model_instance_id: String,
            load_time_seconds: f64,
        },
        #[serde(rename = "prompt_processing.start")]
        PromptProcessingStart,
        #[serde(rename = "prompt_processing.progress")]
        PromptProcessingProgress { progress: f64 },
        #[serde(rename = "prompt_processing.end")]
        PromptProcessingEnd,
        #[serde(rename = "reasoning.start")]
        ReasoningStart,
        #[serde(rename = "reasoning.delta")]
        ReasoningDelta { content: String },
        #[serde(rename = "reasoning.end")]
        ReasoningEnd,
        #[serde(rename = "tool_call.start")]
        ToolCallStart {
            tool: String,
            provider_info: ProviderInfo,
        },
        #[serde(rename = "tool_call.arguments")]
        ToolCallArguments {
            tool: String,
            arguments: HashMap<String, String>,
            provider_info: ProviderInfo,
        },
        #[serde(rename = "tool_call.success")]
        ToolCallSuccess {
            tool: String,
            arguments: HashMap<String, String>,
            output: String,
            provider_info: ProviderInfo,
        },
        #[serde(rename = "tool_call.failure")]
        ToolCallFailure { reason: String, metadata: Metadata },
        #[serde(rename = "message.start")]
        MessageStart,
        #[serde(rename = "message.delta")]
        MessageDelta { content: String },
        #[serde(rename = "message.end")]
        MessageEnd,
        #[serde(rename = "error")]
        Error { error: StreamError },
        #[serde(rename = "chat.end")]
        ChatEnd { result: ChatResponse },
    }

    #[derive(Deserialize, Debug)]
    pub struct StreamError {
        #[serde(rename = "type")]
        pub error_type: ErrorType,
        pub message: String,
        pub code: Option<String>,
        pub param: Option<String>,
    }

    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "snake_case")]
    pub enum ErrorType {
        InvalidRequest,
        Unknown,
        McpConnectionError,
        PluginConnectionError,
        NotImplemented,
        ModelNotFound,
        JobNotFound,
        InternalError,
        /// An error type this crate does not know yet, kept as sent by the server.
        #[serde(untagged)]
        Other(String),
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn parse_error(data: &str) -> StreamError {
            match serde_json::from_str::<StreamEvent>(data).unwrap() {
                StreamEvent::Error { error } => error,
                other => panic!("expected an error event, got {other:?}"),
            }
        }

        #[test]
        fn error_event_is_parsed() {
            let error = parse_error(
                r#"{"type":"error","error":{"type":"invalid_request","message":"\"model\" is required","code":"missing_required_parameter","param":"model"}}"#,
            );

            assert!(matches!(error.error_type, ErrorType::InvalidRequest));
            assert_eq!(error.message, "\"model\" is required");
            assert_eq!(error.code.as_deref(), Some("missing_required_parameter"));
            assert_eq!(error.param.as_deref(), Some("model"));
        }

        #[test]
        fn error_event_without_optional_fields_is_parsed() {
            let error = parse_error(
                r#"{"type":"error","error":{"type":"model_not_found","message":"no such model"}}"#,
            );

            assert!(matches!(error.error_type, ErrorType::ModelNotFound));
            assert_eq!(error.message, "no such model");
            assert!(error.code.is_none());
            assert!(error.param.is_none());
        }

        #[test]
        fn documented_unknown_error_type_is_parsed() {
            let error =
                parse_error(r#"{"type":"error","error":{"type":"unknown","message":"boom"}}"#);

            assert!(matches!(error.error_type, ErrorType::Unknown));
            assert_eq!(error.message, "boom");
        }

        #[test]
        fn undocumented_error_type_is_kept_as_sent() {
            let error = parse_error(
                r#"{"type":"error","error":{"type":"something_new","message":"boom"}}"#,
            );

            assert!(matches!(error.error_type, ErrorType::Other(ref t) if t == "something_new"));
            assert_eq!(error.message, "boom");
        }
    }
}
