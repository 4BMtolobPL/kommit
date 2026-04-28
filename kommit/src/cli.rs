use crate::prompt::ResponseLang;
use crate::provider::{LlmProvider, ThinkType};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub(crate) struct Args {
    #[arg(long)]
    pub(crate) staged: bool,

    /// Name of the model
    #[arg(short, long, default_value = "gemma4")]
    pub(crate) model: String,

    /// Language to writing commit messages
    #[arg(long, default_value_t = ResponseLang::En)]
    pub(crate) lang: ResponseLang,

    #[arg(long, default_value_t = LlmProvider::Ollama)]
    pub(crate) provider: LlmProvider,

    #[arg(short, long)]
    pub(crate) stream: bool,

    #[arg(short, long)]
    pub(crate) think: Option<ThinkType>,
}
