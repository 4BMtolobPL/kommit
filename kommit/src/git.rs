use anyhow::{Context, bail};
use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;
use tracing::info;

pub(crate) fn get_diff(staged: bool) -> anyhow::Result<String> {
    info!(%staged, "Getting diff");
    // TODO: diff 너무 길면 truncate 필요
    // TODO: binary 파일 제외

    let output = if staged {
        Command::new("git").args(["diff", "--staged"]).output()?
    } else {
        Command::new("git").args(["diff"]).output()?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {}", stderr);
    }

    // output이 비어있을때 처리
    if output.stdout.is_empty() {
        bail!("git diff is empty");
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) fn add_all() -> anyhow::Result<()> {
    info!("Adding all changes");
    Command::new("git").args(["add", "--all"]).status()?;
    Ok(())
}

pub(crate) fn commit(file: &NamedTempFile) -> anyhow::Result<()> {
    info!(file = %file.path().display(), "Committing with message");

    Command::new("git")
        .args(["commit", "-F"])
        .arg(file.path())
        .status()?;
    Ok(())
}

pub(crate) fn push() -> anyhow::Result<()> {
    info!("Pushing");
    Command::new("git").args(["push"]).status()?;
    Ok(())
}

pub(crate) fn save_buffer_to_tempfile(buffer: &[u8]) -> anyhow::Result<NamedTempFile> {
    let mut tempfile = NamedTempFile::new().context("Cannot create new NamedTempFile")?;
    tempfile
        .write_all(buffer)
        .context("Failed to write buffer to tempfile")?;
    Ok(tempfile)
}

pub(crate) fn edit_commit_message(file: &NamedTempFile) -> anyhow::Result<()> {
    info!(file = %file.path().display(), "Edit commit message by editor");

    let editor_env = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    let mut iter = editor_env.split_whitespace();
    let editor_binary = iter.next().unwrap_or("vim");

    let mut cmd = Command::new(editor_binary);
    for arg in iter {
        cmd.arg(arg);
    }

    let status = cmd
        .arg(file.path())
        .status()
        .context("Failed to execute editor process")?;

    if status.success() {
        Ok(())
    } else {
        bail!("Editor exited with non-zero status");
    }
}
