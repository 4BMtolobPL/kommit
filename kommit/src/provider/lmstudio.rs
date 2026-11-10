use crate::provider::{
    LlmStream, ProviderStrategy, StreamResponse, ThinkType, download_style, missing_model_error,
};
use anyhow::{Context, bail};
use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use indicatif::ProgressBar;
use lms_api::chat::request::ChatRequestBuilder;
use lms_api::chat::response::Output;
use lms_api::chat::stream::response::StreamEvent;
use lms_api::models::download::request::DownloadRequestBuilder;
use lms_api::models::response::Model;
use lms_api::types::DownloadStatus;
use lms_api::{LmStudio, models::load::request::LoadRequestBuilder};
use std::collections::HashSet;
use std::fmt::Write;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, instrument, trace};
use url::Url;

/// How often a running download is checked.
const DOWNLOAD_POLL_INTERVAL: Duration = Duration::from_millis(500);

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
        let Some(found) = find_model(&models, model) else {
            return Err(missing_model_error(
                self.name(),
                model,
                &self.download_command(model),
            ));
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

/// Finds `model` by its key or by the id of one of its loaded instances.
fn find_model<'a>(models: &'a [Model], model: &str) -> Option<&'a Model> {
    models.iter().find(|m| {
        m.key == model
            || m.loaded_instances
                .iter()
                .any(|instance| instance.id == model)
    })
}

#[async_trait]
impl ProviderStrategy for LmStudioClient {
    fn name(&self) -> &'static str {
        "LM Studio"
    }

    fn download_command(&self, model: &str) -> String {
        format!("lms get {model}")
    }

    async fn is_model_installed(&self, model: &str) -> anyhow::Result<bool> {
        let models = self
            .lms
            .models()
            .await
            .context("Failed to connect to LmStudio. Is it running?")?;
        Ok(find_model(&models, model).is_some())
    }

    #[instrument(skip(self))]
    async fn download_model(&self, model: &str) -> anyhow::Result<()> {
        info!("Downloading model");
        let request = DownloadRequestBuilder::default()
            .model(model)
            .build()
            .context("failed to build download request")?;
        let started = self
            .lms
            .download(request)
            .await
            .context("Failed to start downloading the model")?;

        eprintln!("Downloading model '{model}' with LM Studio...");
        let bar = ProgressBar::new(started.total_size_bytes.unwrap_or(0))
            .with_style(download_style())
            .with_message("downloading");
        let mut status = started.status;
        loop {
            match status {
                DownloadStatus::Completed | DownloadStatus::AlreadyDownloaded => break,
                DownloadStatus::Downloading | DownloadStatus::Paused => {
                    let Some(job_id) = started.job_id.as_deref() else {
                        bar.abandon();
                        bail!(
                            "LM Studio did not return a job to follow the download of '{model}'."
                        );
                    };
                    tokio::time::sleep(DOWNLOAD_POLL_INTERVAL).await;
                    let job = match self.lms.download_status(job_id).await {
                        Ok(job) => job,
                        Err(e) => {
                            bar.abandon();
                            return Err(e).context("Failed to get the download status");
                        }
                    };
                    status = job.status;
                    if let Some(total) = job.total_size_bytes {
                        bar.set_length(total);
                    }
                    if let Some(downloaded) = job.downloaded_bytes {
                        bar.set_position(downloaded);
                    }
                    bar.set_message(if status == DownloadStatus::Paused {
                        "paused"
                    } else {
                        "downloading"
                    });
                }
                DownloadStatus::Failed => {
                    bar.abandon();
                    bail!("LM Studio failed to download model '{model}'.")
                }
                DownloadStatus::Unknown => {
                    bar.abandon();
                    bail!("LM Studio reported an unknown status while downloading '{model}'.")
                }
            }
        }
        bar.finish();
        eprintln!("Downloaded model '{model}'.");

        // A catalog name may be listed under a different key once downloaded.
        if !self.is_model_installed(model).await? {
            bail!(
                "LM Studio downloaded '{model}', but lists no model with that key. Check the key with `lms ls` and pass it with --model."
            );
        }
        Ok(())
    }

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
