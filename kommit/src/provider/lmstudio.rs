use crate::provider::{LlmStream, ProviderStrategy, StreamResponse, ThinkType};
use anyhow::{Context, bail};
use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use lms_api::chat::request::ChatRequestBuilder;
use lms_api::chat::response::Output;
use lms_api::chat::stream::response::StreamEvent;
use lms_api::{LmStudio, models::load::request::LoadRequestBuilder};
use std::collections::HashSet;
use std::fmt::Write;
use tokio::sync::Mutex;
use tracing::{info, instrument, trace};
use url::Url;

#[derive(Default)]
pub(crate) struct LmStudioClient {
    lms: LmStudio,
    /// Models already confirmed to be loaded during this run.
    ready_models: Mutex<HashSet<String>>,
}

impl LmStudioClient {
    pub(crate) fn new(host: Url, port: u16) -> anyhow::Result<Self> {
        Ok(Self {
            lms: LmStudio::new(host, port)?,
            ready_models: Mutex::default(),
        })
    }

    /// Makes sure `model` has a loaded instance, loading one only if none exists.
    ///
    /// The load endpoint always starts a new instance, so calling it per request would
    /// leave a copy of the model behind for every request.
    async fn ensure_loaded(&self, model: &str) -> anyhow::Result<()> {
        // Held until the model is ready, so that concurrent requests load it at most once.
        let mut ready_models = self.ready_models.lock().await;
        if ready_models.contains(model) {
            return Ok(());
        }

        let models = self
            .lms
            .models()
            .await
            .context("Failed to connect to LmStudio. Is it running?")?;
        let found = models.iter().find(|m| {
            m.key == model
                || m.loaded_instances
                    .iter()
                    .any(|instance| instance.id == model)
        });
        let Some(found) = found else {
            bail!(
                "Model '{model}' was not found in LM Studio. Download it first (e.g. `lms get {model}`) or check the model name."
            );
        };

        if found.loaded_instances.is_empty() {
            info!("Loading model: {model}");
            let request = LoadRequestBuilder::default()
                .model(model)
                .build()
                .context("failed to build load request")?;
            self.lms
                .load(request)
                .await
                .context("failed to load model")?;
        } else {
            info!("Model already loaded: {model}");
        }

        ready_models.insert(model.to_owned());
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

        self.ensure_loaded(model).await?;

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

        self.ensure_loaded(model).await?;

        let mut builder = ChatRequestBuilder::default();
        if let Some(think_type) = think {
            builder.reasoning(think_type);
        }

        let mut stream = self
            .lms
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

                        yield Ok(StreamResponse::ThinkDone);
                    }
                    StreamEvent::MessageDelta { content } => {
                        info!(content = %content, event = "message.delta");

                        yield Ok(StreamResponse::Generate(content));
                    }
                    StreamEvent::Error { error } => {
                        yield Err(anyhow::anyhow!(
                            "LM Studio returned an error: {} ({:?})",
                            error.message,
                            error.error_type
                        ));
                        return;
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
