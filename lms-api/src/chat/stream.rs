use crate::LmStudio;
use crate::chat::request::ChatRequest;
use crate::chat::stream::response::StreamEvent;
use crate::error::ApiError;
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
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        info!("Send a message to a model and receive a response. Supports MCP integration.");

        let url = self.endpoint("api/v1/chat")?;
        let request = request.with_stream(true);
        let res = self.client.post(url).json(&request).send().await?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(ApiError::Status(status, body));
        }

        let s = stream! {
            let mut buffer = Vec::new();
            let mut stream = res.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        buffer.extend_from_slice(&chunk);

                        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                            let line_bytes = buffer[..pos].to_vec();
                            buffer.drain(..pos + 1);

                            let line = String::from_utf8_lossy(&line_bytes);
                            let line = line.trim();

                            if line.is_empty() {
                                continue;
                            }

                            if let Some(ev) = line.strip_prefix("event:") {
                                debug!(event = %ev.trim(), "SSE event type");
                            } else if let Some(data) = line.strip_prefix("data:") {
                                let data = data.trim();
                                if data == "[DONE]" {
                                    return;
                                }

                                match serde_json::from_str::<StreamEvent>(data) {
                                    Ok(response) => yield Ok(response),
                                    Err(e) => {
                                        error!(error = %e, data = %data, "Failed to deserialize response");
                                        yield Err(ApiError::Other(format!("Deserialization error: {}", e)));
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to read response");
                        yield Err(ApiError::Request(e));
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(s))
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
        error_type: ErrorType,
        message: String,
        code: Option<String>,
        param: Option<String>,
    }

    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "snake_case")]
    enum ErrorType {
        InvalidRequest,
        Unknown,
        McpConnectionError,
        PluginConnectionError,
        NotImplemented,
        ModelNotFound,
        JobNotFound,
        InternalError,
    }
}
