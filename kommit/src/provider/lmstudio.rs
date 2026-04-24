use crate::provider::LlmClient;
use anyhow::Context;
use async_trait::async_trait;
use futures::{StreamExt, pin_mut};
use lms_api::LmStudio;
use lms_api::chat::request::ChatRequestBuilder;
use lms_api::chat::response::Output;
use lms_api::chat::stream::response::StreamEvent;
use owo_colors::OwoColorize;
use std::fmt::Write;
use tokio::io::AsyncWriteExt;
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
    async fn generate_stream(&self, model: &str, prompt: &str) -> anyhow::Result<String> {
        info!("Generating message");
        trace!(prompt = prompt, "Generating commit message");

        let lms = LmStudio::default();

        let stream = lms
            .chat_stream(
                ChatRequestBuilder::default()
                    .model(model)
                    .input(prompt)
                    .build()?,
            )
            .await
            .context("Failed to connect to LmStudio. Is it running?")?;

        pin_mut!(stream);

        let mut stdout = tokio::io::stdout();
        while let Some(res) = stream.next().await {
            let responses = res.unwrap();

            info!(stream_event = ?responses, "Stream");

            match responses {
                StreamEvent::ReasoningDelta { content } => {
                    info!(content = %content, event = "reasoning.delta");

                    stdout
                        .write_all(content.bright_black().to_string().as_bytes())
                        .await?;
                    stdout.flush().await?;
                }
                StreamEvent::ReasoningEnd => {
                    stdout.write_all("\n\n\n".as_bytes()).await?;
                    stdout.flush().await?;
                }
                StreamEvent::MessageDelta { content } => {
                    info!(content = %content, event = "message.delta");
                    stdout.write_all(content.as_bytes()).await?;
                    stdout.flush().await?;
                }
                _ => continue,
            }
        }

        Ok("".to_string())
    }
}
