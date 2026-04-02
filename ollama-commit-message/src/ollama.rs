use ollama_rs::Ollama;
use ollama_rs::generation::completion::request::GenerationRequest;
use tracing::{info, trace};

pub(crate) async fn generate(model: &str, prompt: &str) -> anyhow::Result<String> {
    info!("Generating message");
    trace!(model = model, prompt = prompt, "Generating commit message");
    // TODO: stream 지원 -> UX 개선 가능

    let ollama = Ollama::default();

    let res = ollama
        .generate(GenerationRequest::new(model.to_string(), prompt))
        .await?;

    Ok(res.response)
}
