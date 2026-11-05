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
        let mut command = Command::new(env!("CARGO_BIN_EXE_kommit"));
        self.isolate(&mut command, &self.path(relative_cwd));
        command
            .args(["run", "--host", "127.0.0.1", "--port"])
            .arg(self.llm.port.to_string())
            .args(args)
            .output()
            .unwrap()
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

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
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
