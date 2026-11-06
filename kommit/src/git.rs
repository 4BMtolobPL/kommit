use anyhow::{Context, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::{NamedTempFile, TempDir};
use tracing::info;

/// How many excluded files to name before summarizing the rest as a count.
const MAX_LISTED_EXCLUDED_FILES: usize = 10;

/// A snapshot of the repository changes: what the LLM reads and what gets committed.
pub(crate) struct ReviewedChanges {
    pub(crate) diff: String,
    pub(crate) tree: String,
    /// The untracked files included in `diff` and `tree`, always empty for `--staged`.
    pub(crate) untracked: Vec<String>,
}

/// Snapshots the changes of the whole repository in a temporary index, leaving the real one alone.
pub(crate) fn review_changes(
    staged: bool,
    exclude_patterns: &[String],
) -> anyhow::Result<ReviewedChanges> {
    info!(%staged, ?exclude_patterns, "Reviewing changes");

    let root = git_path(&["rev-parse", "--show-toplevel"])?;
    let index = git_path(&["rev-parse", "--git-path", "index"])?;
    let temporary_dir = tempfile::tempdir()?;
    let temporary_index = temporary_dir.path().join("index");
    if index.exists() {
        std::fs::copy(&index, &temporary_index)
            .with_context(|| format!("failed to copy {}", index.display()))?;
    }

    // Before `git add`, which would stage the conflict markers as if they resolved the conflict.
    let conflicted = read_unmerged_file_names(&root, &temporary_index)?;
    if !conflicted.is_empty() {
        bail!(
            "Unresolved merge conflicts: {}. Resolve them and stage the result with git add first.",
            summarize_file_names(&conflicted)
        );
    }

    let mut untracked = Vec::new();
    if !staged {
        // Instead of `git add --all`, add exactly the listed untracked files, so that a file
        // created after the listing cannot slip past the confirmation of that list.
        let untracked_paths = read_untracked_paths(&root, &temporary_index)?;
        let status = git_with_index(&root, &temporary_index)
            .args(["add", "--update"])
            .status()?;
        if !status.success() {
            bail!("git add failed with status {status}");
        }
        if !untracked_paths.is_empty() {
            add_literal_paths(&root, &temporary_index, &untracked_paths)?;
        }
        untracked = untracked_paths
            .iter()
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect();
    }

    let output = git_with_index(&root, &temporary_index)
        .arg("write-tree")
        .output()?;
    if !output.status.success() {
        bail!(
            "git write-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let tree = String::from_utf8(output.stdout)?.trim().to_owned();

    let diff = read_staged_diff(&root, &temporary_index, exclude_patterns)?;
    if diff.is_empty() {
        let changed_files = read_staged_file_names(&root, &temporary_index)?;
        if changed_files.is_empty() {
            bail!("git diff is empty");
        }
        bail!(
            "Only excluded files changed: {}. Use git add and git commit to commit them.",
            summarize_file_names(&changed_files)
        );
    }

    Ok(ReviewedChanges {
        diff,
        tree,
        untracked,
    })
}

/// Lists the untracked files that are not ignored, as raw paths relative to `root`.
fn read_untracked_paths(root: &Path, index: &Path) -> anyhow::Result<Vec<Vec<u8>>> {
    let output = git_with_index(root, index)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git ls-files failed: {}", stderr);
    }

    Ok(output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

/// Adds `paths` to `index`, matching each one literally rather than as a pathspec pattern.
fn add_literal_paths(root: &Path, index: &Path, paths: &[Vec<u8>]) -> anyhow::Result<()> {
    let mut child = git_with_index(root, index)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args(["add", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("git add has no stdin")?;
    let written = paths.iter().try_for_each(|path| {
        stdin.write_all(path)?;
        stdin.write_all(b"\0")
    });
    // Closed before waiting, so that git sees the end of the list.
    drop(stdin);
    let status = child.wait()?;
    written.context("failed to pass the untracked files to git add")?;
    if !status.success() {
        bail!("git add failed with status {status}");
    }
    Ok(())
}

fn git_with_index(root: &Path, index: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root).env("GIT_INDEX_FILE", index);
    command
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
    path_from_git_output(&output.stdout)
}

#[cfg(unix)]
fn path_from_git_output(stdout: &[u8]) -> anyhow::Result<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // Unix paths are arbitrary bytes, so they may be neither UTF-8 nor free of edge whitespace.
    let path = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    Ok(PathBuf::from(OsStr::from_bytes(path)))
}

#[cfg(not(unix))]
fn path_from_git_output(stdout: &[u8]) -> anyhow::Result<PathBuf> {
    Ok(PathBuf::from(std::str::from_utf8(stdout)?.trim()))
}

fn read_staged_diff(
    root: &Path,
    index: &Path,
    exclude_patterns: &[String],
) -> anyhow::Result<String> {
    // Pass pathspec exclusions using git pathspec syntax
    let exclude_args = exclude_patterns
        .iter()
        .map(|pat| format!(":(exclude){}", pat));

    let output = git_with_index(root, index)
        .args(["diff", "--staged", "--", "."])
        .args(exclude_args)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn read_staged_file_names(root: &Path, index: &Path) -> anyhow::Result<Vec<String>> {
    let output = git_with_index(root, index)
        .args(["diff", "--staged", "--name-only", "-z"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {}", stderr);
    }

    Ok(output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

/// Lists the paths with unmerged entries, each once, in index order.
fn read_unmerged_file_names(root: &Path, index: &Path) -> anyhow::Result<Vec<String>> {
    let output = git_with_index(root, index)
        .args(["ls-files", "--unmerged", "-z"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git ls-files failed: {}", stderr);
    }

    // Each record is `<mode> <object> <stage>\t<path>`, repeated for every stage of a path.
    let mut names: Vec<String> = output
        .stdout
        .split(|&byte| byte == 0)
        .filter_map(|record| {
            let tab = record.iter().position(|&byte| byte == b'\t')?;
            Some(String::from_utf8_lossy(&record[tab + 1..]).into_owned())
        })
        .collect();
    names.dedup();
    Ok(names)
}

fn summarize_file_names(names: &[String]) -> String {
    let listed = names[..names.len().min(MAX_LISTED_EXCLUDED_FILES)].join(", ");
    match names.len().saturating_sub(MAX_LISTED_EXCLUDED_FILES) {
        0 => listed,
        hidden => format!("{listed} and {hidden} more"),
    }
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

/// Runs `command` with inherited stdio and fails if it exits unsuccessfully.
/// The command's own stderr has already reached the terminal, so the returned
/// error only names the command and its exit status.
fn run_checked(command: &mut Command, name: &str) -> anyhow::Result<()> {
    let status = command
        .status()
        .with_context(|| format!("Failed to execute {name}"))?;
    if !status.success() {
        bail!("{name} failed: {status}");
    }
    Ok(())
}

/// A copy of the real index taken before it is overwritten, so that a failed commit
/// can put the staging area back exactly as it was, per-entry flags included.
pub(crate) struct IndexBackup {
    index: PathBuf,
    /// `None` when there was no index yet, as in a fresh repository.
    saved: Option<TempDir>,
    head: Option<String>,
}

impl IndexBackup {
    pub(crate) fn save() -> anyhow::Result<Self> {
        let index = std::path::absolute(git_path(&["rev-parse", "--git-path", "index"])?)?;
        let saved = if index.exists() {
            let dir = tempfile::tempdir()?;
            std::fs::copy(&index, dir.path().join("index"))
                .with_context(|| format!("failed to copy {}", index.display()))?;
            Some(dir)
        } else {
            None
        };
        let head = read_head()?;
        Ok(Self { index, saved, head })
    }

    /// Whether a commit was created since the backup, in which case restoring
    /// the old index would put it out of step with the new HEAD.
    pub(crate) fn head_moved(&self) -> anyhow::Result<bool> {
        Ok(read_head()? != self.head)
    }

    /// Puts the saved index back, taking `index.lock` like git does so that it
    /// never overwrites an index another git process is writing.
    pub(crate) fn restore(self) -> anyhow::Result<()> {
        let mut lock = self.index.clone().into_os_string();
        lock.push(".lock");
        let lock = PathBuf::from(lock);
        let mut lock_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .with_context(|| format!("failed to lock {}", self.index.display()))?;

        let result = match &self.saved {
            Some(dir) => std::fs::read(dir.path().join("index"))
                .and_then(|content| lock_file.write_all(&content))
                .and_then(|()| lock_file.sync_all())
                .and_then(|()| {
                    // Closed first, because Windows may refuse to rename an open file.
                    drop(lock_file);
                    std::fs::rename(&lock, &self.index)
                }),
            None => {
                drop(lock_file);
                match std::fs::remove_file(&self.index) {
                    Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                    _ => Ok(()),
                }
            }
        };
        // After a successful rename the lock is already gone.
        if self.saved.is_none() || result.is_err() {
            let _ = std::fs::remove_file(&lock);
        }
        result.with_context(|| format!("failed to restore {}", self.index.display()))
    }
}

fn read_head() -> anyhow::Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()
        .context("Failed to execute git rev-parse")?;
    // `--quiet` makes an unborn HEAD exit with 1 and print nothing.
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

pub(crate) fn apply_tree(tree: &str) -> anyhow::Result<()> {
    // `--reset` keeps per-entry flags such as skip-worktree and assume-unchanged.
    run_checked(
        Command::new("git").args(["read-tree", "--reset", tree]),
        "git read-tree",
    )
}

pub(crate) fn commit(file: &NamedTempFile) -> anyhow::Result<()> {
    info!(file = %file.path().display(), "Committing with message");

    run_checked(
        Command::new("git").args(["commit", "-F"]).arg(file.path()),
        "git commit",
    )
}

pub(crate) fn push() -> anyhow::Result<()> {
    info!("Pushing");
    run_checked(Command::new("git").args(["push"]), "git push")
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
        let diff = review_changes(false, &[]).unwrap().diff;
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
        let ReviewedChanges { diff, tree, .. } = review_changes(false, &[]).unwrap();
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
        let ReviewedChanges { diff, tree, .. } = review_changes(false, &[]).unwrap();
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
        let ReviewedChanges { diff, tree, .. } = review_changes(true, &[]).unwrap();
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
            let ReviewedChanges { diff, tree, .. } = review_changes(false, &[]).unwrap();
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

    fn porcelain_status() -> String {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    fn lock_patterns() -> Vec<String> {
        vec!["*.lock".to_owned()]
    }

    #[test]
    fn staged_review_from_subdirectory_includes_other_directories() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::create_dir(repo.path().join("sub")).unwrap();
        fs::create_dir(repo.path().join("other")).unwrap();
        fs::write(repo.path().join("other/file.txt"), "elsewhere\n").unwrap();
        git(repo.path(), &["add", "other/file.txt"]);

        let _cwd = CwdGuard::enter(&repo.path().join("sub"));
        let diff = review_changes(true, &[]).unwrap().diff;

        assert!(diff.contains("+elsewhere"), "diff was {diff:?}");
    }

    #[test]
    fn only_excluded_changes_are_named_and_keep_the_index() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("Cargo.lock"), "1\n").unwrap();
        git(repo.path(), &["add", "Cargo.lock"]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        fs::write(repo.path().join("Cargo.lock"), "2\n").unwrap();
        let _cwd = CwdGuard::enter(repo.path());

        let error = review_changes(false, &lock_patterns())
            .err()
            .expect("review should fail")
            .to_string();

        assert_eq!(
            error,
            "Only excluded files changed: Cargo.lock. Use git add and git commit to commit them."
        );
        assert_eq!(porcelain_status(), " M Cargo.lock\n");
    }

    #[test]
    fn staged_review_ignores_unstaged_excluded_changes() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("Cargo.lock"), "1\n").unwrap();
        git(repo.path(), &["add", "Cargo.lock"]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        fs::write(repo.path().join("Cargo.lock"), "2\n").unwrap();
        let _cwd = CwdGuard::enter(repo.path());

        let error = review_changes(true, &lock_patterns())
            .err()
            .expect("review should fail")
            .to_string();

        assert_eq!(error, "git diff is empty");
    }

    #[test]
    fn long_excluded_file_lists_are_shortened() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        for number in 10..=21 {
            fs::write(repo.path().join(format!("{number}.lock")), "1\n").unwrap();
        }
        let _cwd = CwdGuard::enter(repo.path());

        let error = review_changes(false, &lock_patterns())
            .err()
            .expect("review should fail")
            .to_string();

        assert!(error.contains("19.lock and 2 more."), "error was {error:?}");
        assert!(!error.contains("20.lock"), "error was {error:?}");
    }

    #[test]
    fn ten_excluded_files_are_all_listed() {
        let names: Vec<String> = (0..10).map(|number| format!("{number}.lock")).collect();
        assert_eq!(summarize_file_names(&names), names.join(", "));
    }

    #[test]
    fn no_changes_report_an_empty_diff() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        let _cwd = CwdGuard::enter(repo.path());

        for staged in [false, true] {
            let error = review_changes(staged, &lock_patterns())
                .err()
                .expect("review should fail")
                .to_string();
            assert_eq!(error, "git diff is empty");
        }
    }

    /// Commits different contents of `file` on two branches and leaves their merge conflicted.
    fn make_conflict(repo: &Path, file: &str) {
        fs::write(repo.join(file), "base\n").unwrap();
        git(repo, &["add", file]);
        git(repo, &["commit", "--quiet", "-m", "base"]);
        git(repo, &["checkout", "--quiet", "-b", "theirs"]);
        fs::write(repo.join(file), "theirs\n").unwrap();
        git(repo, &["commit", "--quiet", "-am", "theirs"]);
        git(repo, &["checkout", "--quiet", "-"]);
        fs::write(repo.join(file), "ours\n").unwrap();
        git(repo, &["commit", "--quiet", "-am", "ours"]);
        let merge = Command::new("git")
            .current_dir(repo)
            .args(["merge", "--quiet", "theirs"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
    }

    #[test]
    fn unresolved_conflict_in_excluded_file_is_reported_as_conflict() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        make_conflict(repo.path(), "Cargo.lock");
        let _cwd = CwdGuard::enter(repo.path());

        for staged in [false, true] {
            let error = review_changes(staged, &lock_patterns())
                .err()
                .expect("review should fail")
                .to_string();

            assert!(
                error.contains("Unresolved merge conflicts: Cargo.lock."),
                "staged = {staged}: error was {error:?}"
            );
        }
    }

    // Other Unix file systems, such as APFS, reject paths that are not valid UTF-8.
    #[cfg(target_os = "linux")]
    #[test]
    fn repository_path_may_be_invalid_utf8() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let _lock = cwd_lock();
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join(OsStr::from_bytes(b"repo-\xff "));
        fs::create_dir(&repo).unwrap();
        init(&repo);
        fs::create_dir(repo.join("sub")).unwrap();
        fs::write(repo.join("file.txt"), "reviewed\n").unwrap();

        let _cwd = CwdGuard::enter(&repo.join("sub"));
        let diff = review_changes(false, &[]).unwrap().diff;

        assert!(diff.contains("+reviewed"), "diff was {diff:?}");
    }

    fn ls_files_tags() -> String {
        let output = Command::new("git")
            .args(["ls-files", "-t"])
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn restore_keeps_skip_worktree_and_partial_staging() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("local.txt"), "shared\n").unwrap();
        fs::write(repo.path().join("file.txt"), "1\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        git(
            repo.path(),
            &["update-index", "--skip-worktree", "local.txt"],
        );
        fs::write(repo.path().join("file.txt"), "2\n").unwrap();
        git(repo.path(), &["add", "file.txt"]);
        fs::write(repo.path().join("file.txt"), "3\n").unwrap();
        fs::write(repo.path().join("new.txt"), "new\n").unwrap();
        let _cwd = CwdGuard::enter(repo.path());
        let status = porcelain_status();
        let tags = ls_files_tags();
        assert!(tags.contains("S local.txt"), "tags were {tags:?}");

        let backup = IndexBackup::save().unwrap();
        let tree = review_changes(false, &[]).unwrap().tree;
        // Without `--reset`, so that the flag would be lost if it were not restored.
        git(repo.path(), &["read-tree", &tree]);
        assert_ne!(porcelain_status(), status);
        assert!(!backup.head_moved().unwrap());
        backup.restore().unwrap();

        assert_eq!(porcelain_status(), status);
        assert_eq!(ls_files_tags(), tags);
        assert!(!repo.path().join(".git/index.lock").exists());
    }

    #[test]
    fn restore_refuses_to_break_an_existing_lock() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("file.txt"), "1\n").unwrap();
        git(repo.path(), &["add", "file.txt"]);
        let _cwd = CwdGuard::enter(repo.path());

        let backup = IndexBackup::save().unwrap();
        git(repo.path(), &["rm", "--quiet", "--cached", "file.txt"]);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        fs::write(repo.path().join(".git/index.lock"), "held").unwrap();

        assert!(backup.restore().is_err());
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(
            fs::read(repo.path().join(".git/index.lock")).unwrap(),
            b"held"
        );
    }

    #[test]
    fn restore_removes_an_index_that_did_not_exist() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("file.txt"), "1\n").unwrap();
        let _cwd = CwdGuard::enter(repo.path());
        assert!(!repo.path().join(".git/index").exists());

        let backup = IndexBackup::save().unwrap();
        git(repo.path(), &["add", "file.txt"]);
        backup.restore().unwrap();

        assert!(!repo.path().join(".git/index").exists());
        assert!(!repo.path().join(".git/index.lock").exists());
        assert_eq!(porcelain_status(), "?? file.txt\n");
    }

    #[test]
    fn review_lists_only_new_unignored_files() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        fs::write(repo.path().join("tracked.txt"), "1\n").unwrap();
        fs::write(repo.path().join(".gitignore"), "ignored.txt\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
        fs::write(repo.path().join("tracked.txt"), "2\n").unwrap();
        fs::write(repo.path().join("ignored.txt"), "ignored\n").unwrap();
        fs::create_dir(repo.path().join("sub")).unwrap();
        fs::write(repo.path().join("sub/new.txt"), "new\n").unwrap();
        let _cwd = CwdGuard::enter(&repo.path().join("sub"));

        let reviewed = review_changes(false, &[]).unwrap();
        assert_eq!(reviewed.untracked, ["sub/new.txt"]);
        assert!(
            reviewed.diff.contains("+new"),
            "diff was {:?}",
            reviewed.diff
        );
        assert!(
            !reviewed.diff.contains("ignored"),
            "diff was {:?}",
            reviewed.diff
        );

        git(repo.path(), &["add", "tracked.txt"]);
        assert!(review_changes(true, &[]).unwrap().untracked.is_empty());
    }

    #[test]
    fn untracked_paths_are_added_literally() {
        let _lock = cwd_lock();
        let repo = tempfile::tempdir().unwrap();
        init(repo.path());
        // `*` would match both files as a pathspec, and `:` would start magic.
        for name in ["a *.txt", ":(exclude)b.txt"] {
            fs::write(repo.path().join(name), "new\n").unwrap();
        }
        let _cwd = CwdGuard::enter(repo.path());

        let reviewed = review_changes(false, &[]).unwrap();
        assert_eq!(reviewed.untracked, [":(exclude)b.txt", "a *.txt"]);
        let output = Command::new("git")
            .args(["ls-tree", "--name-only", "-z", &reviewed.tree])
            .output()
            .unwrap();
        assert_eq!(output.stdout, b":(exclude)b.txt\0a *.txt\0");
    }

    fn quiet(program: &str, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[test]
    fn test_run_checked_success() {
        assert!(run_checked(&mut quiet("git", &["--version"]), "git version").is_ok());
    }

    #[test]
    fn test_run_checked_non_zero_exit() {
        let err = run_checked(
            &mut quiet("git", &["definitely-not-a-command"]),
            "git definitely-not-a-command",
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("git definitely-not-a-command failed"), "{err}");
        assert!(err.contains("exit status"), "{err}");
    }

    #[test]
    fn test_run_checked_missing_binary() {
        let err = run_checked(&mut quiet("definitely-not-a-binary", &[]), "missing")
            .unwrap_err()
            .to_string();

        assert!(err.contains("Failed to execute missing"), "{err}");
    }
}
