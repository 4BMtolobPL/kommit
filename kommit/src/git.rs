use anyhow::{Context, bail};
use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;
use tracing::info;

pub(crate) fn get_diff(staged: bool, exclude_patterns: &[String]) -> anyhow::Result<String> {
    info!(%staged, ?exclude_patterns, "Getting diff");

    let mut args = vec!["diff"];
    if staged {
        args.push("--staged");
    }

    // Pass pathspec exclusions using git pathspec syntax
    args.push("--");
    args.push(".");

    let exclude_args: Vec<String> = exclude_patterns
        .iter()
        .map(|pat| format!(":(exclude){}", pat))
        .collect();

    for arg in &exclude_args {
        args.push(arg);
    }

    let output = Command::new("git").args(&args).output()?;

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

pub(crate) fn get_current_branch() -> anyhow::Result<String> {
    info!("Getting current branch name");
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git rev-parse failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn add_all() -> anyhow::Result<()> {
    info!("Adding all changes");
    let status = Command::new("git").args(["add", "--all"]).status()?;
    if !status.success() {
        bail!("git add failed with status {status}");
    }
    Ok(())
}

pub(crate) fn commit(file: &NamedTempFile) -> anyhow::Result<()> {
    info!(file = %file.path().display(), "Committing with message");

    let status = Command::new("git")
        .args(["commit", "-F"])
        .arg(file.path())
        .status()?;
    if !status.success() {
        bail!("git commit failed with status {status}");
    }
    Ok(())
}

pub(crate) fn push() -> anyhow::Result<()> {
    info!("Pushing");
    let status = Command::new("git").args(["push"]).status()?;
    if !status.success() {
        bail!("git push failed with status {status}");
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    fn cwd_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn git(repo: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn staged_diff_includes_untracked_files_after_add_all() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "--quiet"]);
        git(
            repo.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "Test"]);
        fs::write(repo.path().join("tracked.txt"), "before\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        fs::write(repo.path().join("tracked.txt"), "after\n").unwrap();
        fs::write(repo.path().join(".env"), "API_KEY=secret\n").unwrap();

        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(repo.path()).unwrap();
        add_all().unwrap();
        let diff = get_diff(true, &[]).unwrap();
        std::env::set_current_dir(old).unwrap();

        assert!(diff.contains("tracked.txt"));
        assert!(diff.contains(".env"));
        assert!(diff.contains("API_KEY=secret"));
    }

    #[test]
    fn commit_reports_git_failure() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "--quiet"]);
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(repo.path()).unwrap();
        let message = save_buffer_to_tempfile(b"feat: test").unwrap();
        let result = commit(&message);
        std::env::set_current_dir(old).unwrap();
        assert!(result.is_err());
    }
}
