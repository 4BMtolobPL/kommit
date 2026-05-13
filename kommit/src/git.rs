use anyhow::bail;
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

pub(crate) fn commit(message: &[u8]) -> anyhow::Result<()> {
    info!(message = %String::from_utf8_lossy(message), "Committing with message");

    let mut tmpfile = NamedTempFile::new()?;
    tmpfile.write_all(message)?;

    Command::new("git")
        .args(["commit", "-F"])
        .arg(tmpfile.path())
        .status()?;
    Ok(())
}

pub(crate) fn push() -> anyhow::Result<()> {
    info!("Pushing");
    Command::new("git").args(["push"]).status()?;
    Ok(())
}
