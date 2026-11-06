//! Runs the real binary against a fake Ollama server that records every prompt.

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

const COMMIT_MESSAGE: &str = "feat: test message";

struct FakeLlm {
    port: u16,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl FakeLlm {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&prompts);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                respond(stream.unwrap(), &recorded);
            }
        });
        Self { port, prompts }
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }
}

fn respond(mut stream: TcpStream, prompts: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    let request: Value = serde_json::from_slice(&body).unwrap();
    prompts
        .lock()
        .unwrap()
        .push(request["prompt"].as_str().unwrap().to_owned());

    let response = serde_json::json!({
        "model": "test",
        "created_at": "2026-01-01T00:00:00Z",
        "response": COMMIT_MESSAGE,
        "done": true,
    })
    .to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.len(),
        response
    )
    .unwrap();
}

/// A repository and a home directory isolated from the user's Git and Kommit settings.
struct TestRepo {
    home: TempDir,
    repo: TempDir,
    llm: FakeLlm,
}

impl TestRepo {
    fn new() -> Self {
        let test_repo = Self {
            home: tempfile::tempdir().unwrap(),
            repo: tempfile::tempdir().unwrap(),
            llm: FakeLlm::start(),
        };
        test_repo.git(&["init", "--quiet"]);
        test_repo.git(&["config", "user.email", "test@example.invalid"]);
        test_repo.git(&["config", "user.name", "Test"]);
        test_repo
    }

    /// A repository with `tracked.txt`, `Cargo.lock`, and `other/file.txt` committed.
    fn with_commit() -> Self {
        let test_repo = Self::new();
        test_repo.write("tracked.txt", "base\n");
        test_repo.write("Cargo.lock", "base\n");
        test_repo.write("other/file.txt", "base\n");
        test_repo.git(&["add", "."]);
        test_repo.git(&["commit", "--quiet", "-m", "initial"]);
        test_repo
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.repo.path().join(relative)
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn isolate(&self, command: &mut Command, cwd: &Path) {
        command
            .current_dir(cwd)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env("GIT_CONFIG_GLOBAL", self.home.path().join(".gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("NO_PROXY", "*")
            .env("no_proxy", "*");
    }

    fn git(&self, args: &[&str]) -> String {
        let mut command = Command::new("git");
        self.isolate(&mut command, self.repo.path());
        let output = command.args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn kommit_in(&self, relative_cwd: &str, args: &[&str]) -> Output {
        self.kommit_command(relative_cwd, args).output().unwrap()
    }

    fn kommit_command(&self, relative_cwd: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kommit"));
        self.isolate(&mut command, &self.path(relative_cwd));
        command
            .args(["run", "--host", "127.0.0.1", "--port"])
            .arg(self.llm.port.to_string())
            .args(args);
        command
    }

    fn kommit(&self, args: &[&str]) -> Output {
        self.kommit_in("", args)
    }

    fn run_ok_in(&self, relative_cwd: &str, args: &[&str]) -> String {
        let output = self.kommit_in(relative_cwd, args);
        assert!(output.status.success(), "kommit {args:?}: {output:?}");
        self.llm.prompts().pop().unwrap()
    }

    fn run_ok(&self, args: &[&str]) -> String {
        self.run_ok_in("", args)
    }

    fn status(&self) -> String {
        self.git(&["status", "--porcelain"])
    }

    /// Runs `kommit` with an editor script that receives the message file as `$1`.
    #[cfg(unix)]
    fn kommit_with_editor(&self, script: &[u8], args: &[&str]) -> Output {
        let editor = self.home.path().join("editor.sh");
        fs::write(&editor, script).unwrap();
        self.kommit_command("", args)
            .env("VISUAL", format!("sh {}", editor.display()))
            .output()
            .unwrap()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// Installs a `pre-commit` hook that rejects every commit.
    fn reject_commits(&self) {
        self.write(".git/hooks/pre-commit", "#!/bin/sh\nexit 1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = self.path(".git/hooks/pre-commit");
            fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Adds a bare repository as `origin` and pushes the current branch to it.
    /// The returned directory must outlive the remote's use.
    fn add_remote(&self) -> TempDir {
        let remote = tempfile::tempdir().unwrap();
        let path = remote.path().to_str().unwrap();
        self.git(&["init", "--quiet", "--bare", path]);
        self.git(&["remote", "add", "origin", path]);
        self.git(&["push", "--quiet", "--set-upstream", "origin", "HEAD"]);
        remote
    }

    /// Stages `tracked.txt`, then leaves a different unstaged edit and a new file.
    fn make_mixed_changes(&self) {
        self.write("tracked.txt", "staged line\n");
        self.git(&["add", "tracked.txt"]);
        self.write("other/file.txt", "unstaged line\n");
        self.write("new.txt", "new file line\n");
    }
}

#[test]
fn plain_run_reviews_every_change_and_changes_nothing() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();
    let status = repo.status();
    let head = repo.head();

    let prompt = repo.run_ok(&[]);

    assert!(prompt.contains("+staged line"), "prompt was {prompt}");
    assert!(prompt.contains("+unstaged line"), "prompt was {prompt}");
    assert!(prompt.contains("+new file line"), "prompt was {prompt}");
    assert_eq!(repo.status(), status);
    assert_eq!(repo.head(), head);
}

#[test]
fn plain_run_reviews_only_staged_changes() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "staged line\n");
    repo.git(&["add", "tracked.txt"]);

    let prompt = repo.run_ok(&[]);

    assert!(prompt.contains("+staged line"), "prompt was {prompt}");
    assert_eq!(repo.status(), "M  tracked.txt\n");
}

#[test]
fn plain_run_reviews_only_new_files() {
    let repo = TestRepo::with_commit();
    repo.write("new.txt", "new file line\n");

    let prompt = repo.run_ok(&[]);

    assert!(prompt.contains("+new file line"), "prompt was {prompt}");
    assert_eq!(repo.status(), "?? new.txt\n");
}

#[test]
fn plain_run_works_before_the_first_commit() {
    let repo = TestRepo::new();
    repo.write("new.txt", "new file line\n");

    let prompt = repo.run_ok(&[]);

    assert!(prompt.contains("+new file line"), "prompt was {prompt}");
    assert_eq!(repo.status(), "?? new.txt\n");
}

#[test]
fn runs_from_a_subdirectory_review_the_whole_repository() {
    let repo = TestRepo::with_commit();
    repo.write("sub/file.txt", "here\n");
    repo.write("other/file.txt", "unstaged line\n");
    repo.write("tracked.txt", "staged line\n");
    repo.git(&["add", "tracked.txt"]);

    let plain = repo.run_ok_in("sub", &[]);
    assert!(plain.contains("+staged line"), "prompt was {plain}");
    assert!(plain.contains("+unstaged line"), "prompt was {plain}");
    assert!(plain.contains("+here"), "prompt was {plain}");

    let staged = repo.run_ok_in("sub", &["--staged"]);
    assert!(staged.contains("+staged line"), "prompt was {staged}");
    assert!(!staged.contains("+unstaged line"), "prompt was {staged}");
}

#[test]
fn plain_run_previews_the_prompt_of_commit() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();

    let preview = repo.run_ok(&[]);
    let committed = repo.run_ok(&["--commit"]);

    assert_eq!(preview, committed);
}

#[test]
fn commit_includes_every_change() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();

    repo.run_ok(&["--commit"]);

    assert_eq!(repo.git(&["show", "HEAD:tracked.txt"]), "staged line\n");
    assert_eq!(
        repo.git(&["show", "HEAD:other/file.txt"]),
        "unstaged line\n"
    );
    assert_eq!(repo.git(&["show", "HEAD:new.txt"]), "new file line\n");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        format!("{COMMIT_MESSAGE}\n")
    );
    assert_eq!(repo.status(), "");
}

#[test]
fn staged_commit_leaves_other_changes_uncommitted() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();

    let prompt = repo.run_ok(&["--staged", "--commit"]);

    assert!(prompt.contains("+staged line"), "prompt was {prompt}");
    assert!(!prompt.contains("+unstaged line"), "prompt was {prompt}");
    assert!(!prompt.contains("+new file line"), "prompt was {prompt}");
    assert_eq!(repo.git(&["show", "HEAD:tracked.txt"]), "staged line\n");
    assert_eq!(repo.git(&["show", "HEAD:other/file.txt"]), "base\n");
    assert_eq!(repo.status(), " M other/file.txt\n?? new.txt\n");
}

#[test]
fn only_excluded_changes_stop_before_the_llm() {
    let repo = TestRepo::with_commit();
    repo.write("Cargo.lock", "changed\n");
    repo.git(&["add", "Cargo.lock"]);
    let status = repo.status();
    let head = repo.head();

    let flag_sets: [&[&str]; 5] = [
        &[],
        &["--staged"],
        &["--commit"],
        &["--staged", "--commit"],
        &["--push"],
    ];
    for flags in flag_sets {
        let output = repo.kommit(flags);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{flags:?} should fail");
        assert!(
            stderr.contains("Only excluded files changed: Cargo.lock."),
            "{flags:?}: stderr was {stderr}"
        );
        assert_eq!(repo.llm.prompts(), Vec::<String>::new(), "{flags:?}");
        assert_eq!(repo.status(), status, "{flags:?}");
        assert_eq!(repo.head(), head, "{flags:?}");
    }
}

/// The start of every notice that reprints an unused commit message.
const UNUSED_MESSAGE_NOTICE: &str = "The commit was not created";
const RESTORED_NOTICE: &str =
    "The commit was not created and the staging area was restored. Commit message:";

#[test]
fn rejected_commit_prints_the_message_and_skips_push() {
    let repo = TestRepo::with_commit();
    let _remote = repo.add_remote();
    // An unpushed commit, so that a stray push would move the remote ref.
    repo.write("other/file.txt", "unpushed\n");
    repo.git(&["commit", "--quiet", "--all", "-m", "unpushed"]);
    repo.write("tracked.txt", "changed\n");
    repo.reject_commits();
    let head = repo.head();
    let remote_refs = repo.git(&["ls-remote", "origin"]);

    let output = repo.kommit(&["--commit", "--push"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr was {stderr}");
    assert!(
        stderr.contains(UNUSED_MESSAGE_NOTICE),
        "stderr was {stderr}"
    );
    assert!(stderr.contains(COMMIT_MESSAGE), "stderr was {stderr}");
    assert!(stderr.contains("git commit failed"), "stderr was {stderr}");
    assert_eq!(repo.head(), head);
    assert_eq!(repo.git(&["ls-remote", "origin"]), remote_refs);
}

// The fake editor is a shell script.
#[cfg(unix)]
#[test]
fn rejected_commit_prints_the_edited_message() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    repo.reject_commits();
    let editor = repo.home.path().join("editor.sh");
    fs::write(&editor, "printf 'fix: edited message\\n' > \"$1\"\n").unwrap();

    let output = repo
        .kommit_command("", &["--commit", "--edit"])
        .env("VISUAL", format!("sh {}", editor.display()))
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr was {stderr}");
    assert!(
        stderr.contains("fix: edited message"),
        "stderr was {stderr}"
    );
    assert!(!stderr.contains(COMMIT_MESSAGE), "stderr was {stderr}");
}

#[test]
fn failed_push_keeps_the_commit_and_does_not_print_the_message() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    let head = repo.head();

    let output = repo.kommit(&["--push"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr was {stderr}");
    assert!(stderr.contains("git push failed"), "stderr was {stderr}");
    assert!(
        !stderr.contains(UNUSED_MESSAGE_NOTICE),
        "stderr was {stderr}"
    );
    assert_ne!(repo.head(), head);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        format!("{COMMIT_MESSAGE}\n")
    );
}

#[test]
fn rejected_commit_restores_the_index() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();
    repo.reject_commits();
    let status = repo.status();
    assert_eq!(status, " M other/file.txt\nM  tracked.txt\n?? new.txt\n");
    let head = repo.head();

    let output = repo.kommit(&["--commit"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr was {stderr}");
    assert!(stderr.contains(RESTORED_NOTICE), "stderr was {stderr}");
    assert_eq!(repo.status(), status);
    assert_eq!(repo.head(), head);
}

#[test]
fn rejected_staged_commit_restores_the_index() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();
    // Partially staged, so that the index differs from both HEAD and the work tree.
    repo.write("tracked.txt", "unstaged on top\n");
    repo.reject_commits();
    let status = repo.status();
    assert!(status.contains("MM tracked.txt"), "status was {status}");

    let output = repo.kommit(&["--staged", "--commit"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr was {stderr}");
    assert_eq!(repo.status(), status);
    assert_eq!(repo.git(&["show", ":tracked.txt"]), "staged line\n");
}

#[test]
fn rejected_first_commit_leaves_no_index() {
    let repo = TestRepo::new();
    repo.write("new.txt", "new file line\n");
    repo.reject_commits();
    assert!(!repo.path(".git/index").exists());

    let output = repo.kommit(&["--commit"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr was {stderr}");
    assert!(stderr.contains(RESTORED_NOTICE), "stderr was {stderr}");
    assert!(!repo.path(".git/index").exists());
    assert!(!repo.path(".git/index.lock").exists());
    assert_eq!(repo.status(), "?? new.txt\n");
}

#[cfg(unix)]
#[test]
fn failed_editor_prints_the_edited_message() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();
    let status = repo.status();
    let head = repo.head();

    let output = repo.kommit_with_editor(
        b"printf 'fix: edited before failing\\n' > \"$1\"\nexit 1\n",
        &["--commit", "--edit"],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr was {stderr}");
    assert!(
        stderr.contains("The commit was not created. Commit message:"),
        "stderr was {stderr}"
    );
    assert!(
        stderr.contains("fix: edited before failing"),
        "stderr was {stderr}"
    );
    assert!(
        stderr.contains("Failed to edit commit message"),
        "stderr was {stderr}"
    );
    assert_eq!(repo.status(), status);
    assert_eq!(repo.head(), head);
}

#[cfg(unix)]
#[test]
fn rejected_commit_prints_non_utf8_message_as_is() {
    // "fix: 수정" in EUC-KR.
    const EUC_KR: &[u8] = b"fix: \xbc\xf6\xc1\xa4";
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    repo.reject_commits();

    let output = repo.kommit_with_editor(
        b"printf 'fix: \\274\\366\\301\\244\\n' > \"$1\"\n",
        &["--commit", "--edit"],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr was {stderr}");
    assert!(
        output
            .stderr
            .windows(EUC_KR.len())
            .any(|window| window == EUC_KR),
        "stderr was {stderr}"
    );
    assert!(!stderr.contains(COMMIT_MESSAGE), "stderr was {stderr}");
}
