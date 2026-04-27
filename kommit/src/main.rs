pub mod cli;
pub mod git;
pub mod prompt;
pub mod provider;

use crate::git::get_diff;
use crate::prompt::build_prompt;
use crate::provider::{StreamResponse, create_client};
use clap::Parser;
use futures::StreamExt;
use std::io;
use std::io::Write;
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

    if args.stream {
        let mut stream = client.generate_stream(&args.model, &prompt).await?;

        let mut out = io::stdout().lock();
        while let Some(res) = stream.next().await {
            match res? {
                StreamResponse::Think(text) | StreamResponse::Generate(text) => {
                    out.write_all(text.as_bytes())?;
                    out.flush()?;
                }
            }
        }
        writeln!(out)?;
    } else {
        let message = client.generate(&args.model, &prompt).await?;
        println!("{message}");
    }

    Ok(())
}

fn init_tracing() {
    let subscriber = fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .pretty()
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}
