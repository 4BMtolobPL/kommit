pub mod cli;
pub mod git;
pub mod ollama;
pub mod prompt;

use crate::git::get_diff;
use crate::ollama::generate;
use crate::prompt::build_prompt;
use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let args = cli::Args::parse();

    let diff = get_diff(args.staged)?;
    let prompt = build_prompt(&diff, &args.lang);
    let message = generate(&args.model, &prompt).await?;

    println!("{message}");

    Ok(())
}

fn init_tracing() {
    /*// a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(Level::TRACE)
        // completes the builder.
        .finish();*/

    let subscriber = fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}
