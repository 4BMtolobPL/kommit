use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub(crate) struct Args {
    #[arg(long)]
    pub(crate) staged: bool,

    /// Name of the model
    #[arg(short, long, default_value = "qwen3.5")]
    pub(crate) model: String,

    /// en|ko
    #[arg(long, default_value = "en")]
    pub(crate) lang: String,
}
