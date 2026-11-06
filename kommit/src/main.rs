pub mod cli;
pub mod config;
pub mod git;
pub mod prompt;
pub mod provider;

use crate::cli::{Cli, Commands, ConfigArgs, ConfigItem, ConfigSubcommand, RunArgs};
use crate::config::Config;
use crate::git::{
    IndexBackup, apply_tree, commit, edit_commit_message, get_current_branch, push, review_changes,
    save_buffer_to_tempfile,
};
use crate::prompt::{
    ResponseLang, build_commit_from_summaries_prompt, build_prompt, build_summary_prompt,
};
use crate::provider::{LlmProvider, StreamResponse, create_client};
use anyhow::{Context, anyhow, bail};
use clap::Parser;
use futures::StreamExt;
use owo_colors::OwoColorize;
use std::io::{self, BufRead, IsTerminal, Write};
use tempfile::NamedTempFile;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};
use url::Url;

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

fn init_tracing() {
    let subscriber = fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .pretty()
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}

async fn run(args: RunArgs) -> anyhow::Result<()> {
    let config = Config::load();
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
    let host_str = args.host.or(config.host.clone());
    let port = args
        .port
        .or(config.port)
        .unwrap_or_else(|| provider.default_port());

    let url = if let Some(h) = host_str {
        if h.contains("://") {
            Url::parse(&h)?
        } else {
            Url::parse(&format!("http://{}", h))?
        }
    } else {
        provider.default_host()
    };

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

    let diff_len = diff.len();
    info!(diff_len, "Processing diff");

    let prompt = if diff_len > 15000 {
        println!(
            "{}",
            format!(
                "⚠️ Large diff detected ({} bytes). Switching to 2-Step Generation.",
                diff_len
            )
            .yellow()
        );
        let chunks = split_diff(&diff);
        let summaries = generate_summary_for_chunks(
            client.as_ref(),
            &model,
            chunks,
            lang.clone(),
            think.clone(),
        )
        .await?;
        build_commit_from_summaries_prompt(&summaries, branch.as_deref(), lang.clone())
    } else {
        build_prompt(&diff, branch.as_deref(), lang.clone())
    };

    let mut buffer = Vec::new();

    if stream_enabled {
        let mut stream = client.generate_stream(&model, &prompt, think).await?;
        let mut out = io::stdout().lock();

        while let Some(res) = stream.next().await {
            match res? {
                StreamResponse::Think(text) => {
                    write!(&mut out, "{}", text.bright_black())?;
                }
                StreamResponse::ThinkDone => {
                    writeln!(&mut out, "\n\n")?;
                }
                StreamResponse::Generate(text) => {
                    write!(&mut out, "{}", text)?;
                    write!(&mut buffer, "{}", text)?;
                }
            }
        }
        // eol
        writeln!(out)?;
    } else {
        let message = client.generate(&model, &prompt, think).await?;
        println!("{message}");
        write!(&mut buffer, "{}", message)?;
    }

    if buffer.is_empty() {
        bail!(
            "There is no message. Please check to see if the response was interrupted due to a lack of context."
        )
    }

    let raw_message =
        String::from_utf8(buffer).context("Failed to parse output buffer to string")?;
    let final_message = clean_commit_message(&raw_message);

    let is_valid = validate_commit_message(&final_message);
    if !is_valid {
        println!(
            "{}",
            "⚠️ Warning: The generated message does not follow Conventional Commits format."
                .yellow()
        );
        println!("{}", "Expected format: <type>: <summary> (Types: feat, fix, refactor, docs, test, chore, etc.)".yellow());
    }

    if args.commit || args.push {
        let mut edit_required = args.edit;
        if !is_valid && !args.edit {
            println!(
                "{}",
                "Auto-enabling edit mode so you can correct the format in your editor.".cyan()
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
        "Untracked files will be sent to the LLM and committed:".yellow()
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

    let mut answer = String::new();
    input.read_line(&mut answer)?;
    let answer = answer.trim();
    if answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes") {
        Ok(())
    } else {
        bail!("Aborted: untracked files were not included.")
    }
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
    eprintln!("{}", notice.yellow());
    let mut stderr = io::stderr().lock();
    let _ = stderr
        .write_all(message)
        .and_then(|()| stderr.write_all(b"\n"));
}

fn config(args: ConfigArgs) -> anyhow::Result<()> {
    match args.command {
        ConfigSubcommand::Show => {
            let path = Config::get_config_path();
            let config = Config::load();
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
            let mut config = Config::load();
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

const MAX_CHUNK_BYTES: usize = 12000;

fn truncate_chunk(chunk: String) -> String {
    if chunk.len() <= MAX_CHUNK_BYTES {
        return chunk;
    }
    let end = chunk.floor_char_boundary(MAX_CHUNK_BYTES);
    format!("{}\n...(truncated due to size)...", &chunk[..end])
}

fn split_diff(diff: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current_chunk = String::new();

    for line in diff.lines() {
        if line.starts_with("diff --git ") && !current_chunk.is_empty() {
            chunks.push(current_chunk);
            current_chunk = String::new();
        }
        current_chunk.push_str(line);
        current_chunk.push('\n');
    }
    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }
    chunks
}

async fn generate_summary_for_chunks(
    client: &dyn crate::provider::ProviderStrategy,
    model: &str,
    chunks: Vec<String>,
    lang: ResponseLang,
    think: Option<crate::provider::ThinkType>,
) -> anyhow::Result<String> {
    let mut summaries = Vec::new();
    let total_chunks = chunks.len();
    for (i, chunk) in chunks.into_iter().enumerate() {
        let chunk_to_send = truncate_chunk(chunk);

        let summary_prompt = build_summary_prompt(&chunk_to_send, lang.clone());
        println!("⏳ Summarizing diff chunk {}/{}...", i + 1, total_chunks);
        let summary = client
            .generate(model, &summary_prompt, think.clone())
            .await?;
        summaries.push(summary);
    }
    Ok(summaries.join("\n"))
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
    let title = msg.lines().next().unwrap_or("").trim();
    if title.is_empty() {
        return false;
    }

    let valid_types = [
        "feat", "fix", "refactor", "docs", "test", "chore", "style", "ci", "perf", "build",
    ];

    if let Some(colon_idx) = title.find(':') {
        let prefix = title[..colon_idx].trim();
        let commit_type = if let Some(open_paren) = prefix.find('(') {
            prefix[..open_paren].trim()
        } else {
            prefix
        };

        valid_types.contains(&commit_type)
    } else {
        false
    }
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

    #[test]
    fn test_split_diff_empty() {
        let diff = "";
        let chunks = split_diff(diff);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_split_diff_single() {
        let diff = "diff --git a/src/main.rs b/src/main.rs\nindex 123456..789abc 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n-fn main() {}\n+fn main() { println!(\"hello\"); }";
        let chunks = split_diff(diff);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("diff --git a/src/main.rs"));
    }

    #[test]
    fn test_split_diff_multiple() {
        let diff = "diff --git a/file1.rs b/file1.rs\n+ fn first() {}\ndiff --git a/file2.rs b/file2.rs\n+ fn second() {}";
        let chunks = split_diff(diff);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("first"));
        assert!(chunks[1].contains("second"));
    }

    const TRUNCATION_NOTICE: &str = "\n...(truncated due to size)...";

    fn assert_truncated_to(input: String, expected_len: usize) {
        let expected_body = input[..expected_len].to_string();
        let result = truncate_chunk(input);
        let body = result
            .strip_suffix(TRUNCATION_NOTICE)
            .expect("truncated chunk should end with the notice");
        assert_eq!(body.len(), expected_len);
        assert_eq!(body, expected_body);
    }

    // 12,000 is a multiple of 3, so this limit lands on a boundary of '가'.
    #[test]
    fn test_truncate_chunk_multibyte_on_boundary() {
        assert_truncated_to("가".repeat(5000), 12000);
    }

    // Reproduces issue #38: byte 12,000 is inside '가' (bytes 11998..12001).
    #[test]
    fn test_truncate_chunk_multibyte_one_byte_offset() {
        assert_truncated_to(format!("a{}", "가".repeat(5000)), 11998);
    }

    #[test]
    fn test_truncate_chunk_multibyte_two_byte_offset() {
        assert_truncated_to(format!("aa{}", "가".repeat(5000)), 11999);
    }

    #[test]
    fn test_truncate_chunk_four_byte_char() {
        assert_truncated_to(format!("a{}", "😀".repeat(3000)), 11997);
    }

    #[test]
    fn test_truncate_chunk_ascii_over_limit() {
        assert_truncated_to("a".repeat(12001), 12000);
    }

    #[test]
    fn test_truncate_chunk_at_limit_is_unchanged() {
        let input = "a".repeat(12000);
        assert_eq!(truncate_chunk(input.clone()), input);
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
    }
}
