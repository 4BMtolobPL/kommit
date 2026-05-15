use crate::prompt::ResponseLang;
use crate::provider::{LlmProvider, ThinkType};
use clap::{Args as ClapArgs, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Generate a commit message based on diff
    Run(RunArgs),
    /// Manage configuration
    Config(ConfigArgs),
}

#[derive(ClapArgs, Debug)]
pub(crate) struct RunArgs {
    /// Summarize only staged changes
    #[arg(long)]
    pub(crate) staged: bool,

    /// Name of the model
    #[arg(short, long)]
    pub(crate) model: Option<String>,

    /// Language for writing commit messages
    #[arg(long)]
    pub(crate) lang: Option<ResponseLang>,

    /// LLM provider
    #[arg(long)]
    pub(crate) provider: Option<LlmProvider>,

    /// Enable or disable streaming response
    #[arg(short, long, value_name = "BOOL")]
    pub(crate) stream: Option<bool>,

    /// Thinking type or level
    #[arg(short, long, value_name = "TYPE")]
    pub(crate) think: Option<ThinkType>,

    /// Commit the changes after generating the message
    #[arg(short, long)]
    pub(crate) commit: bool,

    /// Push the changes after committing
    #[arg(short, long)]
    pub(crate) push: bool,

    /// Host of the LLM server
    #[arg(long, value_name = "HOST")]
    pub(crate) host: Option<String>,

    /// Port of the LLM server
    #[arg(long, value_name = "PORT")]
    pub(crate) port: Option<u16>,

    /// Edit the generated commit message before committing
    #[arg(short, long)]
    pub(crate) edit: bool,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct ConfigArgs {
    #[command(subcommand)]
    pub(crate) command: ConfigSubcommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum ConfigSubcommand {
    /// Show the current configuration and path
    Show,
    /// Set a configuration value
    Set {
        #[command(subcommand)]
        item: ConfigItem,
    },
    /// Open the configuration directory in the default file manager
    Open,
}

#[derive(Subcommand, Debug)]
pub(crate) enum ConfigItem {
    Model {
        value: String,
    },
    Lang {
        value: ResponseLang,
    },
    Provider {
        value: LlmProvider,
    },
    Stream {
        #[arg(value_parser = ["true", "false"])]
        value: String,
    },
    Think {
        value: ThinkType,
    },
    Host {
        value: String,
    },
    Port {
        value: u16,
    },
}
