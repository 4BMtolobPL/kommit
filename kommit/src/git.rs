use anyhow::{Context, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::NamedTempFile;
use tracing::info;

pub(crate) fn get_diff(staged: bool, exclude_patterns: &[String]) -> anyhow::Result<String> {
    get_diff_with_index(staged, exclude_patterns, None)
}

pub(crate) fn get_diff_for_commit(
    staged: bool,
    exclude_patterns: &[String],
) -> anyhow::Result<(String, String)> {
    let root = git_path(&["rev-parse", "--show-toplevel"])?;
    let index = git_path(&["rev-parse", "--git-path", "index"])?;
    let temporary_dir = tempfile::tempdir()?;
    let temporary_index = temporary_dir.path().join("index");
    if index.exists() {
        std::fs::copy(&index, &temporary_index)
            .with_context(|| format!("failed to copy {}", index.display()))?;
    }

    if !staged {
        let status = Command::new("git")
            .current_dir(&root)
            .env("GIT_INDEX_FILE", &temporary_index)
            .args(["add", "--all"])
            .status()?;
        if !status.success() {
            bail!("git add failed with status {status}");
        }
    }

    let diff = get_diff_with_index_at(true, exclude_patterns, Some(&temporary_index), Some(&root))?;
    let output = Command::new("git")
        .current_dir(&root)
        .env("GIT_INDEX_FILE", &temporary_index)
        .arg("write-tree")
        .output()?;
    if !output.status.success() {
        bail!(
            "git write-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok((diff, String::from_utf8(output.stdout)?.trim().to_owned()))
}

fn git_path(args: &[&str]) -> anyhow::Result<PathBuf> {
    let output = Command::new("git").args(args).output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
}

fn get_diff_with_index(
    staged: bool,
    exclude_patterns: &[String],
    index: Option<&Path>,
) -> anyhow::Result<String> {
    get_diff_with_index_at(staged, exclude_patterns, index, None)
}

fn get_diff_with_index_at(
    staged: bool,
    exclude_patterns: &[String],
    index: Option<&Path>,
    root: Option<&Path>,
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
    if let Some(root) = root {
        command.current_dir(root);
    }
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

pub(crate) fn apply_tree(tree: &str) -> anyhow::Result<()> {
    // `--reset` keeps per-entry flags such as skip-worktree and assume-unchanged.
    let status = Command::new("git")
        .args(["read-tree", "--reset", tree])
        .status()?;
    if !status.success() {
        bail!("git read-tree failed with status {status}");
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

    struct CwdGuard(PathBuf);
    impl CwdGuard {
        fn enter(path: &Path) -> Self {
            let old = std::env::current_dir().unwrap();
            std::env::set_current_dir(path).unwrap();
            Self(old)
        }
    }
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).unwrap();
        }
    }

    fn init(repo: &Path) {
        git(repo, &["init", "--quiet"]);
        git(repo, &["config", "core.excludesFile", ""]);
        git(repo, &["config", "core.hooksPath", "/dev/null"]);
        git(repo, &["config", "commit.gpgSign", "false"]);
        git(repo, &["config", "user.email", "test@example.invalid"]);
        git(repo, &["config", "user.name", "Test"]);
    }

    #[test]
    fn commit_diff_does_not_modify_real_index() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("tracked.txt"), "before\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        fs::write(repo.path().join("tracked.txt"), "staged\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        fs::write(repo.path().join("tracked.txt"), "unstaged\n").unwrap();
        fs::write(repo.path().join("new.txt"), "new\n").unwrap();

        let _cwd = CwdGuard::enter(repo.path());
        let (diff, _) = get_diff_for_commit(false, &[]).unwrap();
        let status_before = Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .unwrap();

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
        init(repo.path());
        let _cwd = CwdGuard::enter(repo.path());
        let message = save_buffer_to_tempfile(b"feat: test").unwrap();
        let result = commit(&message);
        assert!(result.is_err());
    }

    #[test]
    fn reviewed_tree_is_committed_from_subdirectory_in_unborn_repo() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::create_dir(repo.path().join("sub")).unwrap();
        fs::write(repo.path().join("root.txt"), "reviewed\n").unwrap();
        let _cwd = CwdGuard::enter(&repo.path().join("sub"));
        let (diff, tree) = get_diff_for_commit(false, &[]).unwrap();
        assert!(diff.contains("+reviewed"));
        fs::write(repo.path().join("root.txt"), "later\n").unwrap();
        apply_tree(&tree).unwrap();
        commit(&save_buffer_to_tempfile(b"test: reviewed").unwrap()).unwrap();
        let output = Command::new("git")
            .args(["show", "HEAD:root.txt"])
            .output()
            .unwrap();
        assert_eq!(output.stdout, b"reviewed\n");
        assert_eq!(fs::read(repo.path().join("root.txt")).unwrap(), b"later\n");
    }

    #[test]
    fn commit_diff_works_in_linked_worktree() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("file.txt"), "before\n").unwrap();
        git(repo.path(), &["add", "file.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        let worktrees = tempfile::tempdir().unwrap();
        let linked = worktrees.path().join("linked");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "linked",
                linked.to_str().unwrap(),
            ],
        );
        fs::write(linked.join("file.txt"), "after\n").unwrap();
        let _cwd = CwdGuard::enter(&linked);
        let (diff, tree) = get_diff_for_commit(false, &[]).unwrap();
        assert!(diff.contains("+after"));
        apply_tree(&tree).unwrap();
        commit(&save_buffer_to_tempfile(b"test: linked").unwrap()).unwrap();
    }

    #[test]
    fn staged_review_commits_the_original_index() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("file.txt"), "reviewed\n").unwrap();
        git(repo.path(), &["add", "file.txt"]);
        let _cwd = CwdGuard::enter(repo.path());
        let (diff, tree) = get_diff_for_commit(true, &[]).unwrap();
        assert!(diff.contains("+reviewed"));
        fs::write(repo.path().join("file.txt"), "later\n").unwrap();
        git(repo.path(), &["add", "file.txt"]);
        apply_tree(&tree).unwrap();
        commit(&save_buffer_to_tempfile(b"test: staged").unwrap()).unwrap();
        let output = Command::new("git")
            .args(["show", "HEAD:file.txt"])
            .output()
            .unwrap();
        assert_eq!(output.stdout, b"reviewed\n");
    }

    #[test]
    fn skip_worktree_changes_stay_out_of_later_commits() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("file.txt"), "1\n").unwrap();
        fs::write(repo.path().join("local.txt"), "shared\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        git(
            repo.path(),
            &["update-index", "--skip-worktree", "local.txt"],
        );
        fs::write(repo.path().join("local.txt"), "local only\n").unwrap();
        let _cwd = CwdGuard::enter(repo.path());

        // The flag must survive the first commit to protect the second one.
        for content in ["2\n", "3\n"] {
            fs::write(repo.path().join("file.txt"), content).unwrap();
            let (diff, tree) = get_diff_for_commit(false, &[]).unwrap();
            assert!(!diff.contains("local only"));
            apply_tree(&tree).unwrap();
            commit(&save_buffer_to_tempfile(b"test: skip worktree").unwrap()).unwrap();
        }

        let output = Command::new("git")
            .args(["show", "HEAD:local.txt"])
            .output()
            .unwrap();
        assert_eq!(output.stdout, b"shared\n");
    }
}
