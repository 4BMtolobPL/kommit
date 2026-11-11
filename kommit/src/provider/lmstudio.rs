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

        let models = self.lms.models().await.map_err(map_lms_error)?;
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

/// The context length `model` is loaded with: that of the instance it names, or else the
/// smallest of its instances, since a request may go to any of them.
fn loaded_context_length(model: &Model, name: &str) -> Option<u32> {
    let instances = &model.loaded_instances;
    match instances.iter().find(|instance| instance.id == name) {
        Some(instance) => Some(instance.config.context_length),
        None => instances
            .iter()
            .map(|instance| instance.config.context_length)
            .min(),
    }
}

/// Turns an LM Studio API error into a message for the user.
///
/// Only a failure to reach the server suggests that LM Studio is not running; an error
/// the server sent back is shown with its own explanation.
fn map_lms_error(err: lms_api::Error) -> anyhow::Error {
    match err {
        lms_api::Error::Http(e) if e.is_connect() || e.is_timeout() => {
            anyhow::anyhow!("Failed to connect to LM Studio. Is it running? (Error: {e})")
        }
        lms_api::Error::Server { status, body } => match server_message(&body) {
            Some(message) => anyhow::anyhow!("LM Studio returned an error ({status}): {message}"),
            None => anyhow::anyhow!("LM Studio returned an error ({status})"),
        },
        other => anyhow::anyhow!(other),
    }
}

/// The explanation in an error body: `{"error": {"message": ...}}`, `{"error": "..."}`,
/// or the body itself when it is not one of those.
fn server_message(body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        let error = &value["error"];
        if let Some(message) = error["message"].as_str().or_else(|| error.as_str()) {
            return Some(message.to_owned());
        }
    }
    Some(body.to_owned())
}

/// The error for a stream that closed without finishing the response, which LM Studio
/// does when the prompt does not fit in the context the model is loaded with.
fn unfinished_stream_error(model: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "LM Studio closed the stream before the response finished. The prompt was likely too long for the context length the model is loaded with. Load the model with a larger context (e.g. `lms load {model} --context-length 8192`) and try again."
    )
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
        let models = self.lms.models().await.map_err(map_lms_error)?;
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

    /// The context length of the loaded instance. A request never sets one, since LM Studio
    /// would then reload the model at that length.
    async fn context_length(&self, model: &str) -> anyhow::Result<Option<u32>> {
        self.ensure_loaded(model).await?;
        let models = self.lms.models().await.map_err(map_lms_error)?;
        Ok(find_model(&models, model).and_then(|found| loaded_context_length(found, model)))
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
            .map_err(map_lms_error)?;

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
            .map_err(map_lms_error)?;

        let model = model.to_owned();
        let s = stream! {
            let mut finished = false;
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
                    StreamEvent::ChatEnd { .. } => finished = true,
                    _ => continue,
                }
            }
            if !finished {
                yield Err(unfinished_stream_error(&model));
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

#[cfg(test)]
mod tests {
    use super::*;
    use lms_api::error::StatusCode;

    fn status_error(body: &str) -> String {
        map_lms_error(lms_api::Error::Server {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: body.to_owned(),
        })
        .to_string()
    }

    fn model_with_instances(contexts: &[(&str, u32)]) -> Model {
        let instances: Vec<serde_json::Value> = contexts
            .iter()
            .map(|(id, context_length)| {
                serde_json::json!({ "id": id, "config": { "context_length": context_length } })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "type": "llm",
            "publisher": "test",
            "key": "model",
            "display_name": "Model",
            "size_bytes": 1,
            "loaded_instances": instances,
            "max_context_length": 131072,
        }))
        .unwrap()
    }

    #[test]
    fn an_instance_id_uses_that_instance() {
        let model = model_with_instances(&[("model", 8192), ("model:2", 4096)]);
        assert_eq!(loaded_context_length(&model, "model"), Some(8192));
        assert_eq!(loaded_context_length(&model, "model:2"), Some(4096));
    }

    #[test]
    fn a_key_uses_the_smallest_instance() {
        let model = model_with_instances(&[("model:2", 8192), ("model:3", 4096)]);
        assert_eq!(loaded_context_length(&model, "model"), Some(4096));
    }

    #[test]
    fn an_unloaded_model_has_no_context_length() {
        let model = model_with_instances(&[]);
        assert_eq!(loaded_context_length(&model, "model"), None);
    }

    #[test]
    fn error_object_message_is_shown() {
        let message = status_error(r#"{"error":{"type":"unknown","message":"too many tokens"}}"#);
        assert_eq!(
            message,
            "LM Studio returned an error (500 Internal Server Error): too many tokens"
        );
    }

    #[test]
    fn error_string_is_shown() {
        let message = status_error(r#"{"error":"too many tokens"}"#);
        assert!(message.ends_with(": too many tokens"), "{message}");
    }

    #[test]
    fn plain_body_is_shown_as_sent() {
        let message = status_error("  too many tokens\n");
        assert!(message.ends_with(": too many tokens"), "{message}");
    }

    #[test]
    fn empty_body_shows_only_the_status() {
        assert_eq!(
            status_error(""),
            "LM Studio returned an error (500 Internal Server Error)"
        );
    }
}
