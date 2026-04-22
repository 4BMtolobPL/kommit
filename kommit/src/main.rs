pub mod cli;
pub mod git;
pub mod prompt;
pub mod provider;

use crate::git::get_diff;
use crate::prompt::build_prompt;
use crate::provider::create_client;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let args = cli::Args::parse();
    info!(?args, "Parsed arguments");

    let client = create_client(args.provider);

    let diff = get_diff(args.staged)?;
    let prompt = build_prompt(&diff, args.lang);
    let message = client.generate(&args.model, &prompt).await?;

    println!("{message}");

    Ok(())
}

fn init_tracing() {
    let subscriber = fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}
