use crate::provider::{LlmStream, ProviderStrategy, StreamResponse, ThinkType};
use anyhow::Context;
use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use lms_api::chat::request::ChatRequestBuilder;
use lms_api::chat::response::Output;
use lms_api::chat::stream::response::StreamEvent;
use lms_api::{LmStudio, models::load::request::LoadRequestBuilder};
use std::fmt::Write;
use tracing::{info, instrument, trace};
use url::Url;

#[derive(Default)]
pub(crate) struct LmStudioClient {
    lms: LmStudio,
}

impl LmStudioClient {
    pub(crate) fn new(host: Url, port: u16) -> anyhow::Result<Self> {
        Ok(Self {
            lms: LmStudio::new(host, port)?,
        })
    }

    async fn load_model(&self, model: &str) -> anyhow::Result<()> {
        info!("Loading model: {model}");

        let request = LoadRequestBuilder::default()
            .model(model)
            .build()
            .context("failed to build load request")?;
        self.lms
            .load(request)
            .await
            .context("failed to load model")?;
        Ok(())
    }
}

#[async_trait]
impl ProviderStrategy for LmStudioClient {
    #[instrument(skip(self, prompt))]
    async fn generate(
        &self,
        model: &str,
        prompt: &str,
        think: Option<ThinkType>,
    ) -> anyhow::Result<String> {
        info!("Generating commit message");
        trace!(prompt = prompt, "Generating commit message");

        self.load_model(model).await?;

        let mut builder = ChatRequestBuilder::default();
        if let Some(think_type) = think {
            builder.reasoning(think_type);
        }

        let res = self
            .lms
            .chat(builder.model(model).input(prompt).build()?)
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
    async fn generate_stream(
        &self,
        model: &str,
        prompt: &str,
        think: Option<ThinkType>,
    ) -> anyhow::Result<LlmStream> {
        info!("Generating commit message");
        trace!(prompt = prompt, "Generating commit message");

        let lms = LmStudio::default();
        let mut builder = ChatRequestBuilder::default();
        if let Some(think_type) = think {
            builder.reasoning(think_type);
        }

        let mut stream = lms
            .chat_stream(builder.model(model).input(prompt).build()?)
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

                        yield Ok(StreamResponse::Think(content));
                    }
                    StreamEvent::ReasoningEnd => {

                        yield Ok(StreamResponse::Think("\n\n\n".to_string()));
                    }
                    StreamEvent::MessageDelta { content } => {
                        info!(content = %content, event = "message.delta");

                        yield Ok(StreamResponse::Generate(content));
                    }
                    _ => continue,
                }
            }
        };

        Ok(Box::pin(s))
    }
}

impl From<ThinkType> for lms_api::types::AllowedOptions {
    fn from(value: ThinkType) -> Self {
        match value {
            ThinkType::True => lms_api::types::AllowedOptions::On,
            ThinkType::False => lms_api::types::AllowedOptions::Off,
            ThinkType::Low => lms_api::types::AllowedOptions::Low,
            ThinkType::Medium => lms_api::types::AllowedOptions::Medium,
            ThinkType::High => lms_api::types::AllowedOptions::High,
        }
    }
}
