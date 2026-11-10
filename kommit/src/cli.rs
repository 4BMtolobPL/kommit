use crate::prompt::ResponseLang;
use crate::provider::{LlmProvider, ThinkType};
use clap::{ArgGroup, Args as ClapArgs, Parser, Subcommand};

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
#[command(group(ArgGroup::new("publish").args(["commit", "push"]).multiple(true)))]
pub(crate) struct RunArgs {
    /// Summarize only staged changes
    #[arg(long)]
    pub(crate) staged: bool,

    /// Name of the model
    #[arg(short, long)]
    pub(crate) model: Option<String>,

    /// Language for writing commit messages
    #[arg(short, long)]
    pub(crate) lang: Option<ResponseLang>,

    /// LLM provider
    #[arg(long)]
    pub(crate) provider: Option<LlmProvider>,

    /// Enable or disable streaming response (`--stream` alone means true)
    #[arg(
        short,
        long,
        value_name = "BOOL",
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub(crate) stream: Option<bool>,

    /// Thinking type or level
    #[arg(short, long, value_name = "TYPE")]
    pub(crate) think: Option<ThinkType>,

    /// Commit the changes after generating the message
    #[arg(short, long)]
    pub(crate) commit: bool,

    /// Commit and push the changes (implies --commit)
    #[arg(short, long)]
    pub(crate) push: bool,

    /// Host of the LLM server
    #[arg(long, value_name = "HOST")]
    pub(crate) host: Option<String>,

    /// Port of the LLM server
    #[arg(long, value_name = "PORT")]
    pub(crate) port: Option<u16>,

    /// Edit the generated commit message before committing (requires --commit or --push)
    #[arg(short, long, requires = "publish")]
    pub(crate) edit: bool,

    /// Include untracked files without asking for confirmation
    #[arg(short, long)]
    pub(crate) yes: bool,

    /// Download the model without asking if it is not installed
    #[arg(long)]
    pub(crate) pull: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_run(args: &[&str]) -> Result<RunArgs, clap::Error> {
        let argv = ["kommit", "run"].iter().chain(args).copied();
        match Cli::try_parse_from(argv)?.command {
            Commands::Run(run) => Ok(run),
            Commands::Config(_) => unreachable!("parsed the run subcommand"),
        }
    }

    #[test]
    fn stream_without_a_value_enables_streaming() {
        assert_eq!(parse_run(&["--stream"]).unwrap().stream, Some(true));
        assert_eq!(parse_run(&["-s"]).unwrap().stream, Some(true));
    }

    #[test]
    fn stream_accepts_an_explicit_value() {
        assert_eq!(parse_run(&["--stream", "true"]).unwrap().stream, Some(true));
        assert_eq!(
            parse_run(&["--stream", "false"]).unwrap().stream,
            Some(false)
        );
        assert_eq!(parse_run(&["-s", "false"]).unwrap().stream, Some(false));
    }

    #[test]
    fn stream_is_unset_when_omitted() {
        assert_eq!(parse_run(&[]).unwrap().stream, None);
    }

    #[test]
    fn stream_without_a_value_does_not_consume_the_next_flag() {
        let args = parse_run(&["-s", "--commit"]).unwrap();
        assert_eq!(args.stream, Some(true));
        assert!(args.commit);
    }

    #[test]
    fn stream_rejects_a_non_boolean_value() {
        assert!(parse_run(&["--stream", "yes"]).is_err());
    }

    #[test]
    fn pull_is_separate_from_yes() {
        let args = parse_run(&["--pull"]).unwrap();
        assert!(args.pull);
        assert!(!args.yes);
        assert!(!parse_run(&["--yes"]).unwrap().pull);
        assert!(!parse_run(&[]).unwrap().pull);
    }

    #[test]
    fn lang_accepts_the_short_option() {
        assert!(matches!(
            parse_run(&["-l", "ko"]).unwrap().lang,
            Some(ResponseLang::Ko)
        ));
        assert!(matches!(
            parse_run(&["--lang", "en"]).unwrap().lang,
            Some(ResponseLang::En)
        ));
    }
}
