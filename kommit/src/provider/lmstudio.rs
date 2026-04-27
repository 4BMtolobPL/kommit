use crate::provider::{LlmClient, LlmStream, StreamResponse};
use anyhow::Context;
use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use lms_api::LmStudio;
use lms_api::chat::request::ChatRequestBuilder;
use lms_api::chat::response::Output;
use lms_api::chat::stream::response::StreamEvent;
use owo_colors::OwoColorize;
use std::fmt::Write;
use tracing::{info, instrument, trace};

pub(crate) struct LmStudioClient {}

impl LmStudioClient {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl LlmClient for LmStudioClient {
    #[instrument(skip(self, prompt))]
    async fn generate(&self, model: &str, prompt: &str) -> anyhow::Result<String> {
        info!("Generating message");
        trace!(prompt = prompt, "Generating commit message");

        let lms = LmStudio::default();

        let res = lms
            .chat(
                ChatRequestBuilder::default()
                    .model(model)
                    .input(prompt)
                    .build()?,
            )
            .await
            .context("Failed to connect to LmStudio. Is it running?")?;

        let output = res.output;

        let mut s = String::new();
        for o in output {
            match o {
                Output::Message { content } => {
                    write!(s, "{content}")?;
                }
                Output::Reasoning { content } => {
                    info!(content = %content, "Reasoning");
                }
                Output::ToolCall { .. } | Output::InvalidToolCall { .. } => continue,
            }
        }

        Ok(s)
    }

    #[instrument(skip(self, prompt))]
    async fn generate_stream(&self, model: &str, prompt: &str) -> anyhow::Result<LlmStream> {
        info!("Generating message");
        trace!(prompt = prompt, "Generating commit message");

        let lms = LmStudio::default();

        let mut stream = lms
            .chat_stream(
                ChatRequestBuilder::default()
                    .model(model)
                    .input(prompt)
                    .build()?,
            )
            .await
            .context("Failed to connect to LmStudio. Is it running?")?;

        let s = stream! {
            while let Some(res) = stream.next().await {
                let responses = match res {
                    Ok(r) => r,
                    Err(e) => {
                        yield Err(anyhow::anyhow!(e));
                        continue;
                    }
                };

                info!(stream_event = ?responses, "Stream");

                match responses {
                    StreamEvent::ReasoningDelta { content } => {
                        info!(content = %content, event = "reasoning.delta");

                        yield Ok(StreamResponse::Think(content.bright_black().to_string()));
                    }
                    StreamEvent::ReasoningEnd => {

                        yield Ok(StreamResponse::Think("\n\n\n".to_string()));
                    }
                    StreamEvent::MessageDelta { content } => {
                        info!(content = %content, event = "message.delta");

                        yield Ok(StreamResponse::Generate(content.to_string()));
                    }
                    _ => continue,
                }
            }
        };

        Ok(Box::pin(s))
    }
}
