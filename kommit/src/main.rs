pub mod budget;
pub mod chunk;
pub mod cli;
pub mod config;
pub mod endpoint;
pub mod git;
pub mod prompt;
pub mod provider;

use crate::budget::InputBudget;
use crate::chunk::{chunk_files, split_diff, truncate_chunk};
use crate::cli::{Cli, Commands, ConfigArgs, ConfigItem, ConfigSubcommand, RunArgs};
use crate::config::Config;
use crate::endpoint::resolve_endpoint;
use crate::git::{
    IndexBackup, apply_tree, commit, edit_commit_message, get_current_branch, push, review_changes,
    save_buffer_to_tempfile,
};
use crate::prompt::{
    COMMIT_TYPES, ResponseLang, build_commit_from_summaries_prompt, build_prompt,
    build_summary_prompt,
};
use crate::provider::{
    LlmProvider, ProviderStrategy, StreamResponse, create_client, missing_model_error,
};
use anyhow::{Context, anyhow, bail};
use clap::Parser;
use futures::StreamExt;
use owo_colors::{OwoColorize, Stream};
use std::io::{self, BufRead, IsTerminal, Write};
use tempfile::NamedTempFile;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => run(args).await?,
        Commands::Config(args) => config(args)?,
    }

    Ok(())
}

/// Logs go to stderr, so that stdout carries only the commit message.
fn init_tracing() {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let subscriber = fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal() && !no_color)
        .pretty()
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}

async fn run(args: RunArgs) -> anyhow::Result<()> {
    let config = Config::load()?;
    info!(?args, ?config, "Running with merged config");

    let provider = args
        .provider
        .or(config.provider.clone())
        .unwrap_or(LlmProvider::Ollama);
    let model = args
        .model
        .or(config.model.clone())
        .unwrap_or_else(|| "gemma4".to_string());
    let lang = args
        .lang
        .or(config.lang.clone())
        .unwrap_or(ResponseLang::En);
    let stream_enabled = args.stream.or(config.stream).unwrap_or(false);
    let think = args.think.or(config.think.clone());
    let context_length = args.context_length.or(config.context_length);
    let (url, port) = resolve_endpoint(
        &provider,
        args.host.as_deref(),
        args.port,
        config.host.as_deref(),
        config.port,
    )?;

    let client = create_client(provider, url, port)?;

    let exclude_patterns = config.get_exclude_patterns();
    // Review a snapshot of the whole repository without touching the real index.
    let reviewed = review_changes(args.staged, &exclude_patterns)?;
    // Before anything reaches the LLM, which may be a remote server.
    if !reviewed.untracked.is_empty() && !args.yes {
        let stdin = io::stdin();
        confirm_untracked(&reviewed.untracked, stdin.is_terminal(), &mut stdin.lock())?;
    }
    let diff = reviewed.diff;
    let branch = get_current_branch().ok();

    // Only once there is something to send, so that nothing is downloaded in vain.
    prepare_model(client.as_ref(), &model, args.pull).await?;
    let budget = decide_budget(client.as_ref(), &model, context_length).await?;

    let diff_len = diff.len();
    info!(diff_len, ?budget, "Processing diff");

    let prompt = if diff_len > budget.prompt_bytes {
        warn(&format!(
            "⚠️ Large diff detected ({diff_len} bytes, over {} for one prompt). Switching to 2-Step Generation.",
            budget.prompt_bytes
        ));
        let files = split_diff(&diff);
        let file_count = files.len();
        let chunks = chunk_files(files, budget.chunk_bytes);
        info!(
            files = file_count,
            chunks = chunks.len(),
            "Chunked the diff"
        );
        let summaries = generate_summary_for_chunks(
            client.as_ref(),
            &model,
            chunks,
            file_count,
            lang.clone(),
            think.clone(),
            budget,
        )
        .await?;
        build_commit_from_summaries_prompt(&summaries, branch.as_deref(), lang.clone())
    } else {
        build_prompt(&diff, branch.as_deref(), lang.clone())
    };

    let mut buffer = Vec::new();
    // On a terminal the streamed tokens already show the message, so it is not printed
    // again. Otherwise stdout is captured, and only the cleaned message may reach it.
    let streamed_to_stdout = stream_enabled && io::stdout().is_terminal();

    if stream_enabled {
        let mut stream = client.generate_stream(&model, &prompt, think).await?;
        // A preview on stderr when stdout is captured, so the user still sees progress.
        let mut tokens: Box<dyn Write> = if streamed_to_stdout {
            Box::new(io::stdout())
        } else {
            Box::new(io::stderr())
        };
        let mut thinking = io::stderr();

        while let Some(res) = stream.next().await {
            match res? {
                StreamResponse::Think(text) => {
                    let text = text.if_supports_color(Stream::Stderr, |text| text.bright_black());
                    write!(thinking, "{text}")?;
                    thinking.flush()?;
                }
                StreamResponse::ThinkDone => {
                    writeln!(thinking, "\n\n")?;
                }
                StreamResponse::Generate(text) => {
                    // Flushed per token, since stdout is line-buffered.
                    write!(tokens, "{text}")?;
                    tokens.flush()?;
                    buffer.extend_from_slice(text.as_bytes());
                }
            }
        }
        // eol
        writeln!(tokens)?;
    } else {
        let message = client.generate(&model, &prompt, think).await?;
        buffer.extend_from_slice(message.as_bytes());
    }

    if buffer.is_empty() {
        bail!(
            "There is no message. Please check to see if the response was interrupted due to a lack of context."
        )
    }

    let raw_message =
        String::from_utf8(buffer).context("Failed to parse output buffer to string")?;
    let final_message = clean_commit_message(&raw_message);
    if !streamed_to_stdout {
        writeln!(io::stdout(), "{final_message}")?;
    }

    let is_valid = validate_commit_message(&final_message);
    if !is_valid {
        warn("⚠️ Warning: The generated message does not follow Conventional Commits format.");
        warn(&format!(
            "Expected format: <type>[(<scope>)][!]: <summary> (Types: {})",
            COMMIT_TYPES.join(", ")
        ));
    }

    if args.commit || args.push {
        let mut edit_required = args.edit;
        if !is_valid && !args.edit {
            eprintln!(
                "{}",
                "Auto-enabling edit mode so you can correct the format in your editor."
                    .if_supports_color(Stream::Stderr, |text| text.cyan())
            );
            edit_required = true;
        }

        let tempfile = save_buffer_to_tempfile(final_message.as_bytes())
            .context("Failed to save buffer to tempfile")?;

        if edit_required
            && let Err(err) =
                edit_commit_message(&tempfile).context("Failed to edit commit message")
        {
            // The index has not been touched yet.
            print_unused_message(&tempfile, &final_message, false);
            return Err(err);
        }

        let backup = IndexBackup::save().context("Failed to back up the index")?;
        let result = apply_tree(&reviewed.tree).and_then(|()| commit(&tempfile));

        if let Err(err) = result {
            match backup.head_moved() {
                // The commit exists despite the failure, so the index matches it already.
                Ok(true) => return Err(err),
                Ok(false) => {
                    let restored = match backup.restore() {
                        Ok(()) => true,
                        Err(restore_err) => {
                            eprintln!("Failed to restore the index: {restore_err:#}");
                            false
                        }
                    };
                    print_unused_message(&tempfile, &final_message, restored);
                }
                Err(head_err) => {
                    eprintln!(
                        "Failed to check whether the commit was created, so the index was left as is: {head_err:#}"
                    );
                }
            }
            return Err(err);
        }
    }

    if args.push {
        push()?;
    }

    Ok(())
}

/// Lists the untracked files on stderr and asks whether to include them, failing unless
/// the answer is yes. Every file is named, since a hidden one could be the secret.
fn confirm_untracked(
    files: &[String],
    is_terminal: bool,
    input: &mut impl BufRead,
) -> anyhow::Result<()> {
    if !is_terminal {
        bail!(
            "Untracked files would be sent to the LLM and committed: {}. Pass --yes to include them, or use --staged.",
            files.join(", ")
        );
    }

    let mut stderr = io::stderr().lock();
    writeln!(
        stderr,
        "{}",
        "Untracked files will be sent to the LLM and committed:"
            .if_supports_color(Stream::Stderr, |text| text.yellow())
    )?;
    for file in files {
        writeln!(stderr, "  {file}")?;
    }
    let question = match files.len() {
        1 => "Include this untracked file?".to_owned(),
        count => format!("Include these {count} untracked files?"),
    };
    write!(stderr, "{question} [y/N] ")?;
    stderr.flush()?;
    drop(stderr);

    if read_yes(input)? {
        Ok(())
    } else {
        bail!("Aborted: untracked files were not included.")
    }
}

/// Makes sure `model` is installed, downloading it with `--pull` or once the user agrees.
async fn prepare_model(
    client: &dyn ProviderStrategy,
    model: &str,
    pull: bool,
) -> anyhow::Result<()> {
    if client.is_model_installed(model).await? {
        return Ok(());
    }
    if !pull {
        let stdin = io::stdin();
        confirm_download(
            client.name(),
            model,
            &client.download_command(model),
            stdin.is_terminal(),
            &mut stdin.lock(),
        )?;
    }
    client.download_model(model).await
}

/// The budget for `model`, sized by the context length the user set, or else the one the
/// provider reports. Without either, the fixed default.
async fn decide_budget(
    client: &dyn ProviderStrategy,
    model: &str,
    configured: Option<u32>,
) -> anyhow::Result<InputBudget> {
    let (context_length, source) = match configured {
        Some(tokens) => (Some(tokens), "configured"),
        None => (client.context_length(model).await?, "provider"),
    };
    let budget = context_length.map_or_else(InputBudget::default, InputBudget::from_context_length);
    info!(?context_length, source, ?budget, "Sized the input budget");
    Ok(budget)
}

/// Asks on stderr whether to download a missing model, failing unless the answer is yes.
/// Without a terminal it fails right away, since the download may take several gigabytes.
fn confirm_download(
    provider: &str,
    model: &str,
    download_command: &str,
    is_terminal: bool,
    input: &mut impl BufRead,
) -> anyhow::Result<()> {
    if !is_terminal {
        return Err(missing_model_error(provider, model, download_command));
    }

    let mut stderr = io::stderr().lock();
    writeln!(
        stderr,
        "{}",
        format!("Model '{model}' is not installed in {provider}.")
            .if_supports_color(Stream::Stderr, |text| text.yellow())
    )?;
    write!(
        stderr,
        "Download it now? This may take several gigabytes. [y/N] "
    )?;
    stderr.flush()?;
    drop(stderr);

    if read_yes(input)? {
        Ok(())
    } else {
        bail!("Aborted: model '{model}' was not downloaded.")
    }
}

/// Reads one answer to a `[y/N]` question, which is yes only for "y" or "yes".
fn read_yes(input: &mut impl BufRead) -> io::Result<bool> {
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    let answer = answer.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

/// Prints the message that failed to be committed so the user can reuse it.
/// The tempfile takes precedence because it holds any edits made by the user,
/// and it is deleted when the process exits. Its bytes are printed as they are,
/// since the user may write in an encoding other than UTF-8.
fn print_unused_message(tempfile: &NamedTempFile, fallback: &str, index_restored: bool) {
    let edited = std::fs::read(tempfile.path()).unwrap_or_default();
    let edited = edited.trim_ascii();
    let message = if edited.is_empty() {
        fallback.trim().as_bytes()
    } else {
        edited
    };

    let notice = if index_restored {
        "The commit was not created and the staging area was restored. Commit message:"
    } else {
        "The commit was not created. Commit message:"
    };
    warn(notice);
    let mut stderr = io::stderr().lock();
    let _ = stderr
        .write_all(message)
        .and_then(|()| stderr.write_all(b"\n"));
}

/// Prints a warning on stderr, in yellow when stderr supports colors.
fn warn(message: &str) {
    eprintln!(
        "{}",
        message.if_supports_color(Stream::Stderr, |text| text.yellow())
    );
}

fn config(args: ConfigArgs) -> anyhow::Result<()> {
    match args.command {
        ConfigSubcommand::Show => {
            let path = Config::get_config_path();
            let config = Config::load()?;
            println!("----------------------------------------");
            println!(
                "Config path: {}",
                path.map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Unknown".to_string())
            );
            println!("----------------------------------------");
            println!("{}", toml::to_string_pretty(&config)?);
            println!("----------------------------------------");
        }
        ConfigSubcommand::Set { item } => {
            let mut config = Config::load()?;
            match item {
                ConfigItem::Model { value } => config.model = Some(value),
                ConfigItem::Lang { value } => config.lang = Some(value),
                ConfigItem::Provider { value } => config.provider = Some(value),
                ConfigItem::Stream { value } => {
                    config.stream = Some(
                        value
                            .parse::<bool>()
                            .map_err(|_| anyhow!("Invalid boolean value"))?,
                    );
                }
                ConfigItem::Think { value } => config.think = Some(value),
                ConfigItem::Host { value } => config.host = Some(value),
                ConfigItem::Port { value } => config.port = Some(value),
                ConfigItem::ContextLength { value } => config.context_length = Some(value),
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
                bail!("Could not find config directory");
            }
        }
    }

    Ok(())
}

/// The most a summary is asked to take, however few chunks there are.
const SUMMARY_MAX_CHARS: usize = 1500;
/// The least a summary is asked to take, so that it can still name its change.
const SUMMARY_MIN_CHARS: usize = 80;

/// How long each of `chunks` summaries may be, so that together they fit one prompt.
/// Few chunks leave room for detailed summaries; only many chunks make them short.
fn summary_allowance(budget: InputBudget, chunks: usize) -> usize {
    (budget.prompt_bytes / chunks.max(1)).clamp(SUMMARY_MIN_CHARS, SUMMARY_MAX_CHARS)
}

async fn generate_summary_for_chunks(
    client: &dyn crate::provider::ProviderStrategy,
    model: &str,
    chunks: Vec<String>,
    files: usize,
    lang: ResponseLang,
    think: Option<crate::provider::ThinkType>,
    budget: InputBudget,
) -> anyhow::Result<String> {
    let mut summaries = Vec::new();
    let total_chunks = chunks.len();
    let allowance = summary_allowance(budget, total_chunks);
    for (i, chunk) in chunks.into_iter().enumerate() {
        let chunk_to_send = truncate_chunk(chunk, budget.chunk_bytes);

        let summary_prompt = build_summary_prompt(&chunk_to_send, lang.clone(), allowance);
        eprintln!("⏳ Summarizing diff chunk {}/{}...", i + 1, total_chunks);
        let summary = client
            .generate(model, &summary_prompt, think.clone())
            .await?;
        summaries.push(summary);
    }
    let summaries = summaries.join("\n");
    check_summaries_fit(&summaries, files, budget)?;
    Ok(summaries)
}

/// Fails when the joined summaries are too large for one prompt, before the model
/// rejects the request. Nothing is cut, so the message never silently misses a change.
fn check_summaries_fit(summaries: &str, files: usize, budget: InputBudget) -> anyhow::Result<()> {
    if summaries.len() <= budget.prompt_bytes {
        return Ok(());
    }
    bail!(
        "The summaries of the {files} changed files total {} bytes, more than the {} bytes one prompt can take. Commit fewer files at a time (e.g. stage some of them and use --staged).",
        summaries.len(),
        budget.prompt_bytes
    )
}

fn clean_commit_message(msg: &str) -> String {
    let mut cleaned = msg.trim().to_string();

    if cleaned.starts_with("```") {
        let lines: Vec<&str> = cleaned.lines().collect();
        if lines.len() > 2 {
            let start = if lines[0].starts_with("```") { 1 } else { 0 };
            let end = if lines[lines.len() - 1].starts_with("```") {
                lines.len() - 1
            } else {
                lines.len()
            };
            cleaned = lines[start..end].join("\n").trim().to_string();
        }
    }

    if cleaned.len() >= 2
        && ((cleaned.starts_with('"') && cleaned.ends_with('"'))
            || (cleaned.starts_with('\'') && cleaned.ends_with('\'')))
    {
        cleaned = cleaned[1..cleaned.len() - 1].trim().to_string();
    }

    cleaned
}

fn validate_commit_message(msg: &str) -> bool {
    // title = type [ "(" scope ")" ] [ "!" ] ":" SP summary
    let title = msg.lines().next().unwrap_or("").trim();

    let type_end = title
        .find(|c: char| !c.is_ascii_lowercase())
        .unwrap_or(title.len());
    let (commit_type, mut rest) = title.split_at(type_end);
    if !COMMIT_TYPES.contains(&commit_type) {
        return false;
    }

    if let Some(after_open) = rest.strip_prefix('(') {
        let Some(close) = after_open.find(')') else {
            return false;
        };
        let scope = &after_open[..close];
        if scope.trim().is_empty() || scope.contains('(') {
            return false;
        }
        rest = &after_open[close + 1..];
    }
    rest = rest.strip_prefix('!').unwrap_or(rest);

    let Some(summary) = rest.strip_prefix(':') else {
        return false;
    };
    summary.starts_with(char::is_whitespace) && !summary.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirm(input: &[u8], is_terminal: bool) -> anyhow::Result<()> {
        let files = [".env".to_owned()];
        confirm_untracked(&files, is_terminal, &mut io::Cursor::new(input))
    }

    #[test]
    fn untracked_files_are_included_only_on_yes() {
        for input in [&b"y\n"[..], b"YES\n", b" Yes "] {
            assert!(confirm(input, true).is_ok(), "{input:?}");
        }
        for input in [&b"n\n"[..], b"\n", b"", b"yess\n"] {
            let error = confirm(input, true).unwrap_err().to_string();
            assert_eq!(error, "Aborted: untracked files were not included.");
        }
    }

    #[test]
    fn untracked_files_without_a_terminal_need_yes() {
        let error = confirm(b"y\n", false).unwrap_err().to_string();
        assert!(error.contains(".env"), "{error}");
        assert!(error.contains("--yes"), "{error}");
    }

    fn confirm_model(input: &[u8], is_terminal: bool) -> anyhow::Result<()> {
        confirm_download(
            "Ollama",
            "missing",
            "ollama pull missing",
            is_terminal,
            &mut io::Cursor::new(input),
        )
    }

    #[test]
    fn missing_model_is_downloaded_only_on_yes() {
        for input in [&b"y\n"[..], b"YES\n", b" Yes "] {
            assert!(confirm_model(input, true).is_ok(), "{input:?}");
        }
        for input in [&b"n\n"[..], b"\n", b"", b"yess\n"] {
            let error = confirm_model(input, true).unwrap_err().to_string();
            assert_eq!(error, "Aborted: model 'missing' was not downloaded.");
        }
    }

    #[test]
    fn missing_model_without_a_terminal_needs_pull() {
        let error = confirm_model(b"y\n", false).unwrap_err().to_string();
        assert!(
            error.contains("Model 'missing' was not found in Ollama"),
            "{error}"
        );
        assert!(error.contains("--pull"), "{error}");
        assert!(error.contains("ollama pull missing"), "{error}");
    }

    #[test]
    fn summary_allowance_shrinks_only_with_many_chunks() {
        let budget = InputBudget {
            prompt_bytes: 15000,
            chunk_bytes: 12000,
        };
        assert_eq!(summary_allowance(budget, 0), SUMMARY_MAX_CHARS);
        assert_eq!(summary_allowance(budget, 2), SUMMARY_MAX_CHARS);
        assert_eq!(summary_allowance(budget, 10), 1500);
        assert_eq!(summary_allowance(budget, 100), 150);
        assert_eq!(summary_allowance(budget, 1000), SUMMARY_MIN_CHARS);
    }

    #[test]
    fn summaries_over_the_prompt_budget_fail_with_a_hint() {
        let budget = InputBudget {
            prompt_bytes: 10,
            chunk_bytes: 10,
        };
        assert!(check_summaries_fit("0123456789", 2, budget).is_ok());
        let error = check_summaries_fit("0123456789a", 2, budget)
            .unwrap_err()
            .to_string();
        assert!(error.contains("2 changed files"), "{error}");
        assert!(error.contains("11 bytes"), "{error}");
        assert!(error.contains("10 bytes"), "{error}");
        assert!(error.contains("--staged"), "{error}");
    }

    #[test]
    fn test_clean_commit_message() {
        let msg = "```\nfeat: add hello world\n```";
        assert_eq!(clean_commit_message(msg), "feat: add hello world");

        let msg2 = "  'fix: db memory leak'  ";
        assert_eq!(clean_commit_message(msg2), "fix: db memory leak");

        assert_eq!(clean_commit_message("'"), "'");
        assert_eq!(clean_commit_message("\""), "\"");
    }

    #[test]
    fn test_validate_commit_message() {
        assert!(validate_commit_message("feat: add something"));
        assert!(validate_commit_message(
            "fix(database): solve connection leak"
        ));
        assert!(validate_commit_message("chore: cleanup"));
        assert!(!validate_commit_message("add new feature"));
        assert!(!validate_commit_message("feat-add-something"));
        assert!(!validate_commit_message("invalid_type: test"));

        // Breaking-change marker, scopes, extra types and bodies
        for ok in [
            "feat!: drop old api",
            "fix(db)!: change schema",
            "perf: speed up diff",
            "docs(readme): update",
            "feat: x\n\nbody",
        ] {
            assert!(validate_commit_message(ok), "should accept {ok:?}");
        }

        // Malformed scope, summary, ordering, casing
        for bad in [
            "feat(: ",
            "feat(): x",
            "feat( ): x",
            "feat(a: x",
            "feat:",
            "feat:   ",
            "feat:x",
            "feat!(x): y",
            "feat(a)(b): x",
            "Feat: x",
            "",
        ] {
            assert!(!validate_commit_message(bad), "should reject {bad:?}");
        }
    }
}
