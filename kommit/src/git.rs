use anyhow::{Context, bail};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use tempfile::NamedTempFile;
use tracing::info;

pub(crate) fn get_diff(staged: bool, exclude_patterns: &[String]) -> anyhow::Result<String> {
    get_diff_with_index(staged, exclude_patterns, None)
}

pub(crate) fn get_diff_for_commit(exclude_patterns: &[String]) -> anyhow::Result<String> {
    let index = Path::new(".git/index");
    let temporary_index = NamedTempFile::new_in(".git")?;
    if index.exists() {
        std::fs::copy(index, temporary_index.path())
            .with_context(|| format!("failed to copy {}", index.display()))?;
    }

    let status = Command::new("git")
        .env("GIT_INDEX_FILE", temporary_index.path())
        .args(["add", "--all"])
        .status()?;
    if !status.success() {
        bail!("git add failed with status {status}");
    }

    get_diff_with_index(true, exclude_patterns, Some(temporary_index.path()))
}

fn get_diff_with_index(
    staged: bool,
    exclude_patterns: &[String],
    index: Option<&Path>,
) -> anyhow::Result<String> {
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

    let mut command = Command::new("git");
    command.args(&args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output()?;

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
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    fn cwd_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn git(repo: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    struct GlobalConfigGuard(Option<OsString>);

    impl GlobalConfigGuard {
        fn set(path: &std::path::Path) -> Self {
            let previous = std::env::var_os("GIT_CONFIG_GLOBAL");
            unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", path) };
            Self(previous)
        }
    }

    impl Drop for GlobalConfigGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", value) },
                None => unsafe { std::env::remove_var("GIT_CONFIG_GLOBAL") },
            }
        }
    }

    #[test]
    fn commit_diff_does_not_modify_real_index() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        let global_config = repo.path().join("empty-global-config");
        fs::write(&global_config, "").unwrap();
        let _global_config = GlobalConfigGuard::set(&global_config);
        git(repo.path(), &["init", "--quiet"]);
        git(repo.path(), &["config", "user.email", "test@example.invalid"]);
        git(repo.path(), &["config", "user.name", "Test"]);
        fs::write(repo.path().join("tracked.txt"), "before\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        fs::write(repo.path().join("tracked.txt"), "staged\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        fs::write(repo.path().join("tracked.txt"), "unstaged\n").unwrap();
        fs::write(repo.path().join("new.txt"), "new\n").unwrap();

        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(repo.path()).unwrap();
        let diff = get_diff_for_commit(&[]).unwrap();
        let status_before = Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        std::env::set_current_dir(old).unwrap();

        assert!(diff.contains("tracked.txt"));
        assert!(diff.contains("new.txt"));
        let status = String::from_utf8_lossy(&status_before.stdout);
        assert!(status.contains("MM tracked.txt"), "status was {status:?}");
        assert!(status.contains("?? new.txt"), "status was {status:?}");
    }

    #[test]
    fn commit_reports_git_failure() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        let global_config = repo.path().join("empty-global-config");
        fs::write(&global_config, "").unwrap();
        let _global_config = GlobalConfigGuard::set(&global_config);
        git(repo.path(), &["init", "--quiet"]);
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(repo.path()).unwrap();
        let message = save_buffer_to_tempfile(b"feat: test").unwrap();
        let result = commit(&message);
        std::env::set_current_dir(old).unwrap();
        assert!(result.is_err());
    }
}
