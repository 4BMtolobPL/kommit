pub mod cli;
pub mod config;
pub mod git;
pub mod prompt;
pub mod provider;

use crate::cli::{Cli, Commands, ConfigItem, ConfigSubcommand};
use crate::config::Config;
use crate::git::get_diff;
use crate::prompt::{ResponseLang, build_prompt};
use crate::provider::{LlmProvider, StreamResponse, create_client};
use clap::Parser;
use futures::StreamExt;
use owo_colors::OwoColorize;
use std::io;
use std::io::Write;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => {
            let config = Config::load();
            info!(?args, ?config, "Running with merged config");

            let provider = args
                .provider
                .or(config.provider)
                .unwrap_or(LlmProvider::Ollama);
            let model = args
                .model
                .or(config.model)
                .unwrap_or_else(|| "gemma4".to_string());
            let lang = args.lang.or(config.lang).unwrap_or(ResponseLang::En);
            let stream_enabled = args.stream.or(config.stream).unwrap_or(false);
            let think = args.think.or(config.think);

            let client = create_client(provider);
            let diff = get_diff(args.staged)?;
            let prompt = build_prompt(&diff, lang);

            if stream_enabled {
                let mut stream = client.generate_stream(&model, &prompt, think).await?;
                let mut out = io::stdout().lock();
                while let Some(res) = stream.next().await {
                    match res? {
                        StreamResponse::Think(text) => {
                            out.write_all(text.bright_black().to_string().as_bytes())?;
                            out.flush()?;
                        }
                        StreamResponse::Generate(text) => {
                            out.write_all(text.as_bytes())?;
                            out.flush()?;
                        }
                    }
                }
                writeln!(out)?;
            } else {
                let message = client.generate(&model, &prompt, think).await?;
                println!("{message}");
            }
        }
        Commands::Config(args) => match args.command {
            ConfigSubcommand::Show => {
                let path = Config::get_config_path();
                let config = Config::load();
                println!(
                    "Config path: {}",
                    path.map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                );
                println!("---");
                println!("{}", toml::to_string_pretty(&config)?);
            }
            ConfigSubcommand::Set { item } => {
                let mut config = Config::load();
                match item {
                    ConfigItem::Model { value } => config.model = Some(value),
                    ConfigItem::Lang { value } => config.lang = Some(value),
                    ConfigItem::Provider { value } => config.provider = Some(value),
                    ConfigItem::Stream { value } => {
                        config.stream = Some(
                            value
                                .parse::<bool>()
                                .map_err(|_| anyhow::anyhow!("Invalid boolean value"))?,
                        );
                    }
                    ConfigItem::Think { value } => config.think = Some(value),
                }
                config.save()?;
                println!("Configuration updated successfully.");
            }
            ConfigSubcommand::Open => {
                if let Some(dir) = Config::get_config_dir() {
                    if !dir.exists() {
                        std::fs::create_dir_all(&dir)?;
                    }
                    opener::open(dir)?;
                } else {
                    anyhow::bail!("Could not find config directory");
                }
            }
        },
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
