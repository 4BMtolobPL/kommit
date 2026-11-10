//! Runs the real binary against fake Ollama and LM Studio servers that record requests.

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::process::{Command, Output};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

const COMMIT_MESSAGE: &str = "feat: test message";
/// A streamed response sends `COMMIT_MESSAGE` as these tokens.
const STREAM_TOKENS: [&str; 2] = ["feat: ", "test message"];

/// The only model the fake Ollama server reports as not installed.
const MISSING_MODEL: &str = "missing-model";

/// Holds back the rest of a streamed response after its first token until signaled.
type StreamGate = Arc<Mutex<Option<Receiver<()>>>>;

/// A fake Ollama server with every model installed except `MISSING_MODEL`.
struct FakeOllama {
    port: u16,
    prompts: Arc<Mutex<Vec<String>>>,
    /// How many times a model was looked up.
    show_requests: Arc<Mutex<usize>>,
    /// How many times a model was pulled. Once pulled, `MISSING_MODEL` is installed.
    pull_requests: Arc<Mutex<usize>>,
    gate: StreamGate,
    /// What a non-streamed response returns.
    response: Arc<Mutex<String>>,
}

impl FakeOllama {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&prompts);
        let show_requests = Arc::new(Mutex::new(0));
        let server_show_requests = Arc::clone(&show_requests);
        let pull_requests = Arc::new(Mutex::new(0));
        let server_pull_requests = Arc::clone(&pull_requests);
        let gate = StreamGate::default();
        let server_gate = Arc::clone(&gate);
        let response = Arc::new(Mutex::new(COMMIT_MESSAGE.to_owned()));
        let server_response = Arc::clone(&response);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let response = server_response.lock().unwrap().clone();
                respond_ollama(
                    stream.unwrap(),
                    &recorded,
                    &server_show_requests,
                    &server_pull_requests,
                    &server_gate,
                    &response,
                );
            }
        });
        Self {
            port,
            prompts,
            show_requests,
            pull_requests,
            gate,
            response,
        }
    }

    /// Makes every later non-streamed response return `response`.
    fn respond_with(&self, response: &str) {
        *self.response.lock().unwrap() = response.to_owned();
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    fn show_requests(&self) -> usize {
        *self.show_requests.lock().unwrap()
    }

    fn pull_requests(&self) -> usize {
        *self.pull_requests.lock().unwrap()
    }

    /// Makes the next streamed response wait after its first token until the returned
    /// sender signals, or a timeout passes.
    fn hold_stream_after_first_token(&self) -> Sender<()> {
        let (sender, receiver) = mpsc::channel();
        *self.gate.lock().unwrap() = Some(receiver);
        sender
    }
}

/// Reads one HTTP request, returning its request line ("POST /path") and its JSON body,
/// or `Value::Null` when it has none.
fn read_request(stream: &TcpStream) -> (String, Value) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
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
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    let request_line = request_line
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    (request_line, body)
}

fn write_json(stream: &mut TcpStream, status: &str, body: &Value) {
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
}

fn respond_ollama(
    mut stream: TcpStream,
    prompts: &Mutex<Vec<String>>,
    show_requests: &Mutex<usize>,
    pull_requests: &Mutex<usize>,
    gate: &StreamGate,
    response: &str,
) {
    let (request_line, request) = read_request(&stream);
    if request_line == "POST /api/pull" {
        assert_eq!(request["name"], MISSING_MODEL);
        respond_ollama_pull(stream);
        *pull_requests.lock().unwrap() += 1;
        return;
    }
    if request_line == "POST /api/show" {
        *show_requests.lock().unwrap() += 1;
        let model = request["name"].as_str().unwrap();
        if model == MISSING_MODEL && *pull_requests.lock().unwrap() == 0 {
            let error = serde_json::json!({ "error": format!("model '{model}' not found") });
            write_json(&mut stream, "404 Not Found", &error);
        } else {
            write_json(&mut stream, "200 OK", &serde_json::json!({}));
        }
        return;
    }
    assert_eq!(request_line, "POST /api/generate");

    prompts
        .lock()
        .unwrap()
        .push(request["prompt"].as_str().unwrap().to_owned());

    if request["stream"] == true {
        respond_ollama_streaming(stream, gate);
        return;
    }

    let response = serde_json::json!({
        "model": "test",
        "created_at": "2026-01-01T00:00:00Z",
        "response": response,
        "done": true,
    });
    write_json(&mut stream, "200 OK", &response);
}

/// Sends the statuses of a pull as newline-delimited JSON, ending with success.
fn respond_ollama_pull(mut stream: TcpStream) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let statuses = [
        serde_json::json!({ "status": "pulling manifest" }),
        serde_json::json!({ "status": "pulling layer", "digest": "sha256:1", "total": 2000, "completed": 1000 }),
        serde_json::json!({ "status": "pulling layer", "digest": "sha256:1", "total": 2000, "completed": 2000 }),
        serde_json::json!({ "status": "verifying sha256 digest" }),
        serde_json::json!({ "status": "success" }),
    ];
    for status in statuses {
        writeln!(stream, "{status}").unwrap();
    }
}

/// Sends `STREAM_TOKENS` as newline-delimited JSON, ending the body by closing the connection.
fn respond_ollama_streaming(mut stream: TcpStream, gate: &StreamGate) {
    let line = |response: &str, done: bool| {
        let chunk = serde_json::json!({
            "model": "test",
            "created_at": "2026-01-01T00:00:00Z",
            "response": response,
            "done": done,
        });
        format!("{chunk}\n")
    };

    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let receiver = gate.lock().unwrap().take();
    for (i, token) in STREAM_TOKENS.iter().enumerate() {
        stream.write_all(line(token, false).as_bytes()).unwrap();
        stream.flush().unwrap();
        if i == 0
            && let Some(receiver) = &receiver
        {
            // On timeout, send the rest anyway so that the test fails instead of hanging.
            let _ = receiver.recv_timeout(Duration::from_secs(5));
        }
    }
    stream.write_all(line("", true).as_bytes()).unwrap();
}

const LM_STUDIO_MODEL: &str = "test-model";
/// What LM Studio answers to a prompt longer than the loaded context.
const CONTEXT_OVERFLOW_ERROR: &str =
    "The number of tokens to keep from the initial prompt is greater than the context length.";

/// A fake LM Studio server with a single downloaded model, `LM_STUDIO_MODEL`.
struct FakeLmStudio {
    port: u16,
    state: Arc<Mutex<LmStudioState>>,
}

struct LmStudioState {
    /// Whether `LM_STUDIO_MODEL` has been downloaded.
    downloaded: bool,
    loaded_instances: usize,
    load_requests: usize,
    chat_requests: usize,
    download_requests: usize,
    /// Whether every prompt is too long for the loaded context.
    overflow_context: bool,
}

impl FakeLmStudio {
    fn start(loaded: bool) -> Self {
        Self::start_with(LmStudioState {
            downloaded: true,
            loaded_instances: usize::from(loaded),
            load_requests: 0,
            chat_requests: 0,
            download_requests: 0,
            overflow_context: false,
        })
    }

    /// Starts a server that has not downloaded `LM_STUDIO_MODEL` yet.
    fn start_without_model() -> Self {
        Self::start_with(LmStudioState {
            downloaded: false,
            loaded_instances: 0,
            load_requests: 0,
            chat_requests: 0,
            download_requests: 0,
            overflow_context: false,
        })
    }

    /// Starts a server whose model is loaded with a context too small for any prompt.
    fn start_overflowing() -> Self {
        Self::start_with(LmStudioState {
            downloaded: true,
            loaded_instances: 1,
            load_requests: 0,
            chat_requests: 0,
            download_requests: 0,
            overflow_context: true,
        })
    }

    fn start_with(state: LmStudioState) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(state));
        let server_state = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                respond_lm_studio(stream.unwrap(), &server_state);
            }
        });
        Self { port, state }
    }

    fn load_requests(&self) -> usize {
        self.state.lock().unwrap().load_requests
    }

    fn chat_requests(&self) -> usize {
        self.state.lock().unwrap().chat_requests
    }

    fn download_requests(&self) -> usize {
        self.state.lock().unwrap().download_requests
    }
}

fn respond_lm_studio(mut stream: TcpStream, state: &Mutex<LmStudioState>) {
    let (request_line, request) = read_request(&stream);
    match request_line.as_str() {
        "GET /api/v1/models" if !state.lock().unwrap().downloaded => {
            write_json(&mut stream, "200 OK", &serde_json::json!({ "models": [] }));
        }
        "POST /api/v1/models/download" => {
            assert_eq!(request["model"], LM_STUDIO_MODEL);
            state.lock().unwrap().download_requests += 1;
            let started = serde_json::json!({
                "job_id": "job-1",
                "status": "downloading",
                "total_size_bytes": 2000,
            });
            write_json(&mut stream, "200 OK", &started);
        }
        "GET /api/v1/models/download/status/job-1" => {
            state.lock().unwrap().downloaded = true;
            let finished = serde_json::json!({
                "job_id": "job-1",
                "status": "completed",
                "downloaded_bytes": 2000,
                "total_size_bytes": 2000,
            });
            write_json(&mut stream, "200 OK", &finished);
        }
        "GET /api/v1/models" => {
            let loaded_instances = state.lock().unwrap().loaded_instances;
            let instances: Vec<Value> = (0..loaded_instances)
                .map(|i| {
                    let id = if i == 0 {
                        LM_STUDIO_MODEL.to_owned()
                    } else {
                        format!("{LM_STUDIO_MODEL}:{}", i + 1)
                    };
                    serde_json::json!({ "id": id, "config": { "context_length": 4096 } })
                })
                .collect();
            let models = serde_json::json!({
                "models": [{
                    "type": "llm",
                    "publisher": "test",
                    "key": LM_STUDIO_MODEL,
                    "display_name": "Test Model",
                    "size_bytes": 1,
                    "loaded_instances": instances,
                    "max_context_length": 4096,
                }]
            });
            write_json(&mut stream, "200 OK", &models);
        }
        "POST /api/v1/models/load" => {
            let mut state = state.lock().unwrap();
            state.load_requests += 1;
            state.loaded_instances += 1;
            let loaded = serde_json::json!({
                "type": "llm",
                "instance_id": LM_STUDIO_MODEL,
                "load_time_seconds": 0.1,
                "status": "loaded",
            });
            write_json(&mut stream, "200 OK", &loaded);
        }
        "POST /api/v1/chat" if state.lock().unwrap().overflow_context => {
            state.lock().unwrap().chat_requests += 1;
            // LM Studio rejects a non-streamed request, but starts a stream and closes it.
            if request["stream"] == true {
                let start = serde_json::json!({
                    "type": "chat.start",
                    "model_instance_id": LM_STUDIO_MODEL,
                });
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: chat.start\ndata: {start}\n\n"
                )
                .unwrap();
            } else {
                let error = serde_json::json!({ "error": CONTEXT_OVERFLOW_ERROR });
                write_json(&mut stream, "500 Internal Server Error", &error);
            }
        }
        "POST /api/v1/chat" => {
            {
                let mut state = state.lock().unwrap();
                state.chat_requests += 1;
                // Without a loaded instance, a real server would load one just in time.
                if state.loaded_instances == 0 {
                    state.loaded_instances = 1;
                }
            }
            let result = serde_json::json!({
                "model_instance_id": LM_STUDIO_MODEL,
                "output": [{ "type": "message", "content": COMMIT_MESSAGE }],
                "stats": {
                    "input_tokens": 1.0,
                    "total_output_tokens": 1.0,
                    "reasoning_output_tokens": 0.0,
                    "tokens_per_second": 1.0,
                    "time_to_first_token_seconds": 0.1,
                },
            });
            if request["stream"] == true {
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                let mut events: Vec<Value> = STREAM_TOKENS
                    .iter()
                    .map(|token| serde_json::json!({ "type": "message.delta", "content": token }))
                    .collect();
                events.push(serde_json::json!({ "type": "chat.end", "result": result }));
                for event in events {
                    let event_type = event["type"].as_str().unwrap();
                    write!(stream, "event: {event_type}\ndata: {event}\n\n").unwrap();
                }
            } else {
                write_json(&mut stream, "200 OK", &result);
            }
        }
        other => {
            let error = serde_json::json!({ "error": format!("unexpected request {other}") });
            write_json(&mut stream, "404 Not Found", &error);
        }
    }
}

/// A repository and a home directory isolated from the user's Git and Kommit settings.
struct TestRepo {
    home: TempDir,
    repo: TempDir,
    ollama: FakeOllama,
}

impl TestRepo {
    fn new() -> Self {
        let test_repo = Self {
            home: tempfile::tempdir().unwrap(),
            repo: tempfile::tempdir().unwrap(),
            ollama: FakeOllama::start(),
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
            .arg(self.ollama.port.to_string())
            .args(args);
        command
    }

    /// Runs `kommit run` with only `args`, leaving the host and port to the caller.
    fn kommit_run_raw(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kommit"));
        self.isolate(&mut command, self.repo.path());
        command.arg("run").args(args).output().unwrap()
    }

    /// Runs `kommit config` with `args`.
    fn kommit_config(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kommit"));
        self.isolate(&mut command, self.repo.path());
        command.arg("config").args(args).output().unwrap()
    }

    /// The config file path, as reported by `config show` before any config exists.
    fn config_path(&self) -> PathBuf {
        let output = self.kommit_config(&["show"]);
        assert!(output.status.success(), "config show: {output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let path = stdout
            .lines()
            .find_map(|line| line.strip_prefix("Config path: "))
            .unwrap_or_else(|| panic!("no config path in {stdout}"));
        let path = PathBuf::from(path);
        assert!(
            path.starts_with(self.home.path()),
            "config path {} is outside the test home",
            path.display()
        );
        path
    }

    /// Writes a config file that sets a valid key and an invalid provider.
    fn write_broken_config(&self) -> (PathBuf, Vec<u8>) {
        let path = self.config_path();
        let content = b"model = \"keep-me\"\nprovider = \"invalid\"\n".to_vec();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &content).unwrap();
        (path, content)
    }

    fn kommit(&self, args: &[&str]) -> Output {
        self.kommit_in("", args)
    }

    fn run_ok_in(&self, relative_cwd: &str, args: &[&str]) -> String {
        let output = self.kommit_in(relative_cwd, args);
        assert!(output.status.success(), "kommit {args:?}: {output:?}");
        self.ollama.prompts().pop().unwrap()
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
    /// Changes two files by about 9 KB each, so that the diff takes the 2-step path with
    /// one summary per file but no chunk is truncated.
    fn make_large_diff(&self) {
        let content = format!("{}\n", "x".repeat(80)).repeat(110);
        self.write("tracked.txt", &content);
        self.write("other/file.txt", &content);
    }

    /// Runs `kommit run` against `lm_studio` instead of the fake Ollama server.
    fn kommit_with_lm_studio(&self, lm_studio: &FakeLmStudio, args: &[&str]) -> Output {
        self.kommit_with_lm_studio_port(lm_studio.port, args)
    }

    /// Runs `kommit run` against an LM Studio server on `port`.
    fn kommit_with_lm_studio_port(&self, port: u16, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kommit"));
        self.isolate(&mut command, self.repo.path());
        command
            .args([
                "run",
                "--provider",
                "lm-studio",
                "--host",
                "127.0.0.1",
                "--port",
            ])
            .arg(port.to_string())
            .args(args)
            .output()
            .unwrap()
    }

    fn make_mixed_changes(&self) {
        self.write("tracked.txt", "staged line\n");
        self.git(&["add", "tracked.txt"]);
        self.write("other/file.txt", "unstaged line\n");
        self.write("new.txt", "new file line\n");
    }

    /// Commits different `tracked.txt` contents on two branches and leaves their merge conflicted.
    fn make_conflict(&self) {
        self.git(&["checkout", "--quiet", "-b", "theirs"]);
        self.write("tracked.txt", "theirs\n");
        self.git(&["commit", "--quiet", "--all", "-m", "theirs"]);
        self.git(&["checkout", "--quiet", "-"]);
        self.write("tracked.txt", "ours\n");
        self.git(&["commit", "--quiet", "--all", "-m", "ours"]);
        let mut merge = Command::new("git");
        self.isolate(&mut merge, self.repo.path());
        let output = merge.args(["merge", "--quiet", "theirs"]).output().unwrap();
        assert!(
            !output.status.success(),
            "merge should conflict: {output:?}"
        );
    }
}

#[test]
fn plain_run_reviews_every_change_and_changes_nothing() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();
    let status = repo.status();
    let head = repo.head();

    let prompt = repo.run_ok(&["--yes"]);

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

    let prompt = repo.run_ok(&["--yes"]);

    assert!(prompt.contains("+new file line"), "prompt was {prompt}");
    assert_eq!(repo.status(), "?? new.txt\n");
}

#[test]
fn plain_run_works_before_the_first_commit() {
    let repo = TestRepo::new();
    repo.write("new.txt", "new file line\n");

    let prompt = repo.run_ok(&["--yes"]);

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

    let plain = repo.run_ok_in("sub", &["--yes"]);
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

    let preview = repo.run_ok(&["--yes"]);
    let committed = repo.run_ok(&["--yes", "--commit"]);

    assert_eq!(preview, committed);
}

#[test]
fn commit_includes_every_change() {
    let repo = TestRepo::with_commit();
    repo.make_mixed_changes();

    repo.run_ok(&["--yes", "--commit"]);

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
        assert_eq!(repo.ollama.prompts(), Vec::<String>::new(), "{flags:?}");
        assert_eq!(repo.status(), status, "{flags:?}");
        assert_eq!(repo.head(), head, "{flags:?}");
    }
}

#[test]
fn unresolved_conflicts_stop_before_the_llm() {
    let repo = TestRepo::with_commit();
    repo.make_conflict();
    let status = repo.status();
    let head = repo.head();
    let unmerged = repo.git(&["ls-files", "--unmerged"]);
    assert!(status.contains("UU tracked.txt"), "status was {status}");

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
            stderr.contains("Unresolved merge conflicts: tracked.txt."),
            "{flags:?}: stderr was {stderr}"
        );
        assert_eq!(repo.ollama.prompts(), Vec::<String>::new(), "{flags:?}");
        assert_eq!(repo.status(), status, "{flags:?}");
        assert_eq!(repo.head(), head, "{flags:?}");
        assert_eq!(repo.git(&["ls-files", "--unmerged"]), unmerged, "{flags:?}");
        assert!(repo.path(".git/MERGE_HEAD").exists(), "{flags:?}");
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

// Reproduces issue #42: `--edit` alone used to be ignored silently.
#[test]
fn edit_without_commit_or_push_is_a_usage_error() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    let status = repo.status();
    let head = repo.head();

    let output = repo.kommit(&["--edit"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "stderr was {stderr}");
    assert!(stderr.contains("--commit"), "stderr was {stderr}");
    assert!(stderr.contains("--push"), "stderr was {stderr}");
    assert!(repo.ollama.prompts().is_empty());
    assert_eq!(repo.status(), status);
    assert_eq!(repo.head(), head);
}

#[cfg(unix)]
#[test]
fn edit_with_push_alone_commits_and_pushes_the_edited_message() {
    let repo = TestRepo::with_commit();
    let _remote = repo.add_remote();
    repo.write("tracked.txt", "changed\n");

    let output = repo.kommit_with_editor(
        b"printf 'fix: edited before pushing\\n' > \"$1\"\n",
        &["--push", "--edit"],
    );

    assert!(output.status.success(), "{output:?}");
    let subject = repo.git(&["log", "-1", "--format=%s", "@{upstream}"]);
    assert_eq!(subject.trim(), "fix: edited before pushing");
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

    let output = repo.kommit(&["--yes", "--commit"]);

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

    let output = repo.kommit(&["--yes", "--commit"]);

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
        &["--yes", "--commit", "--edit"],
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

#[test]
fn untracked_files_without_a_terminal_stop_before_the_llm() {
    let repo = TestRepo::with_commit();
    let _remote = repo.add_remote();
    repo.write("tracked.txt", "changed\n");
    repo.write(".env", "API_KEY=secret\n");
    let status = repo.status();
    let head = repo.head();
    let remote_refs = repo.git(&["ls-remote", "origin"]);

    let flag_sets: [&[&str]; 3] = [&[], &["--commit"], &["--push"]];
    for flags in flag_sets {
        let output = repo.kommit(flags);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{flags:?}: stderr was {stderr}"
        );
        assert!(stderr.contains(".env"), "{flags:?}: stderr was {stderr}");
        assert!(stderr.contains("--yes"), "{flags:?}: stderr was {stderr}");
        assert_eq!(repo.ollama.prompts(), Vec::<String>::new(), "{flags:?}");
        assert_eq!(repo.status(), status, "{flags:?}");
        assert_eq!(repo.head(), head, "{flags:?}");
        assert_eq!(repo.git(&["ls-remote", "origin"]), remote_refs, "{flags:?}");
    }
}

#[test]
fn yes_includes_untracked_files() {
    let repo = TestRepo::with_commit();
    repo.write(".env", "API_KEY=secret\n");

    let prompt = repo.run_ok(&["--yes", "--commit"]);

    assert!(prompt.contains("+API_KEY=secret"), "prompt was {prompt}");
    assert_eq!(repo.git(&["show", "HEAD:.env"]), "API_KEY=secret\n");
    assert_eq!(repo.status(), "");
}

#[test]
fn staged_run_does_not_ask_about_untracked_files() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "staged line\n");
    repo.git(&["add", "tracked.txt"]);
    repo.write(".env", "API_KEY=secret\n");

    let prompt = repo.run_ok(&["--staged", "--commit"]);

    assert!(prompt.contains("+staged line"), "prompt was {prompt}");
    assert!(!prompt.contains("API_KEY"), "prompt was {prompt}");
    assert_eq!(repo.status(), "?? .env\n");
}

#[test]
fn untracked_files_with_pathspec_characters_are_committed_literally() {
    let repo = TestRepo::with_commit();
    repo.write("a *.txt", "star\n");
    repo.write(":(exclude)b.txt", "magic\n");

    let prompt = repo.run_ok(&["--yes", "--commit"]);

    assert!(prompt.contains("+star"), "prompt was {prompt}");
    assert!(prompt.contains("+magic"), "prompt was {prompt}");
    assert_eq!(repo.git(&["show", "HEAD:a *.txt"]), "star\n");
    assert_eq!(repo.git(&["show", "HEAD::(exclude)b.txt"]), "magic\n");
    assert_eq!(repo.status(), "");
}

#[test]
fn untracked_files_are_committed_despite_global_pathspec_settings() {
    for variable in ["GIT_GLOB_PATHSPECS", "GIT_ICASE_PATHSPECS"] {
        let repo = TestRepo::with_commit();
        repo.write("new.txt", "new file line\n");

        let output = repo
            .kommit_command("", &["--yes", "--commit"])
            .env(variable, "1")
            .output()
            .unwrap();

        assert!(output.status.success(), "{variable}: {output:?}");
        assert_eq!(repo.git(&["show", "HEAD:new.txt"]), "new file line\n");
    }
}

#[test]
fn config_set_keeps_a_broken_config_file() {
    let repo = TestRepo::new();
    let (path, content) = repo.write_broken_config();

    let output = repo.kommit_config(&["set", "port", "1234"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr was {stderr}");
    assert!(
        stderr.contains(&path.display().to_string()),
        "stderr was {stderr}"
    );
    assert_eq!(fs::read(&path).unwrap(), content);
}

#[test]
fn config_set_keeps_an_unknown_key() {
    let repo = TestRepo::new();
    let path = repo.config_path();
    let content = b"modle = \"typo\"\n";
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();

    let output = repo.kommit_config(&["set", "port", "1234"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr was {stderr}");
    assert!(stderr.contains("modle"), "stderr was {stderr}");
    assert_eq!(fs::read(&path).unwrap(), content);
}

#[test]
fn config_show_reports_a_broken_config_file() {
    let repo = TestRepo::new();
    let (path, _) = repo.write_broken_config();

    let output = repo.kommit_config(&["show"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr was {stderr}");
    assert!(
        stderr.contains(&path.display().to_string()),
        "stderr was {stderr}"
    );
}

#[test]
fn broken_config_stops_run_before_the_llm() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    let (path, _) = repo.write_broken_config();

    let output = repo.kommit(&["--commit"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr was {stderr}");
    assert!(
        stderr.contains(&path.display().to_string()),
        "stderr was {stderr}"
    );
    assert_eq!(repo.ollama.prompts(), Vec::<String>::new());
    assert_eq!(repo.status(), " M tracked.txt\n");
}

#[test]
fn config_set_creates_a_missing_config_file() {
    let repo = TestRepo::new();
    let path = repo.config_path();

    let output = repo.kommit_config(&["set", "port", "1234"]);

    assert!(output.status.success(), "config set: {output:?}");
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("port = 1234"), "config was {content}");
}

#[test]
fn port_in_cli_host_is_used() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    let host = format!("127.0.0.1:{}", repo.ollama.port);

    let output = repo.kommit_run_raw(&["--host", &host]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(repo.ollama.prompts().len(), 1);
}

#[test]
fn port_in_config_host_is_used() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    let path = repo.config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!("host = \"127.0.0.1:{}\"\n", repo.ollama.port),
    )
    .unwrap();

    let output = repo.kommit_run_raw(&[]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(repo.ollama.prompts().len(), 1);
}

#[test]
fn conflicting_ports_stop_before_the_llm() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    let host = format!("127.0.0.1:{}", repo.ollama.port);

    let output = repo.kommit_run_raw(&["--host", &host, "--port", "1"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr was {stderr}");
    assert!(stderr.contains("Conflicting ports"), "stderr was {stderr}");
    assert_eq!(repo.ollama.prompts(), Vec::<String>::new());
}

#[test]
fn large_diff_is_split_per_file_despite_color_config() {
    let repo = TestRepo::with_commit();
    repo.git(&["config", "color.ui", "always"]);
    repo.make_large_diff();

    let output = repo.kommit(&[]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{output:?}");
    assert!(
        stderr.contains("Large diff detected"),
        "stderr was {stderr}"
    );
    assert!(stderr.contains("chunk 1/2"), "stderr was {stderr}");
    assert!(stderr.contains("chunk 2/2"), "stderr was {stderr}");
    assert_eq!(output.stdout, format!("{COMMIT_MESSAGE}\n").as_bytes());
    let prompts = repo.ollama.prompts();
    let summaries = prompts
        .iter()
        .filter(|prompt| prompt.contains("git diff chunk"))
        .count();
    assert_eq!(summaries, 2, "prompts were {prompts:?}");
    for prompt in &prompts {
        assert!(!prompt.contains('\x1b'), "prompt was {prompt:?}");
    }
}

/// Many files make each summary short, in proportion to how many there are.
/// Summaries that still overflow the prompt stop the run before the final request.
#[test]
fn summaries_too_large_for_one_prompt_stop_before_the_final_request() {
    let repo = TestRepo::with_commit();
    let content = format!("{}\n", "y".repeat(80)).repeat(10);
    for i in 0..20 {
        repo.write(&format!("file_{i}.txt"), &content);
    }
    repo.ollama.respond_with(&"- a long summary\n".repeat(60));

    let output = repo.kommit(&["--yes"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        stderr.contains("The summaries of the 20 changed files total"),
        "stderr was {stderr}"
    );
    assert!(stderr.contains("--staged"), "stderr was {stderr}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let prompts = repo.ollama.prompts();
    assert_eq!(prompts.len(), 20, "only summaries were requested");
    for prompt in &prompts {
        assert!(
            prompt.contains("under about 750 characters"),
            "prompt was {prompt}"
        );
    }
}

/// With stdout captured, the tokens are previewed on stderr as they arrive, and stdout
/// receives only the finished message.
#[test]
fn streamed_tokens_are_previewed_on_stderr_before_the_stream_ends() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    let first_token_seen = repo.ollama.hold_stream_after_first_token();

    let mut child = repo
        .kommit_command("", &["--stream", "true"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut printed = Vec::new();
    let mut printed_when_first_token_seen = None;
    let mut chunk = [0; 1024];
    loop {
        let read = stderr.read(&mut chunk).unwrap();
        if read == 0 {
            break;
        }
        printed.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&printed).into_owned();
        if printed_when_first_token_seen.is_none() && text.contains(STREAM_TOKENS[0]) {
            let _ = first_token_seen.send(());
            printed_when_first_token_seen = Some(text);
        }
    }
    let output = child.wait_with_output().unwrap();

    let printed = String::from_utf8_lossy(&printed);
    assert!(output.status.success(), "stderr was {printed}");
    assert!(printed.contains(COMMIT_MESSAGE), "stderr was {printed}");
    // The server sends the second token only after the first one appears on stderr.
    let early = printed_when_first_token_seen.unwrap();
    assert!(
        !early.contains(STREAM_TOKENS[1]),
        "stderr was {early:?} when the first token appeared"
    );
    assert_eq!(output.stdout, format!("{COMMIT_MESSAGE}\n").as_bytes());
}

// Issue #49: stdout carries only the commit message, so that it can be captured.
#[test]
fn stdout_is_only_the_cleaned_message() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    repo.ollama
        .respond_with(&format!("```\n{COMMIT_MESSAGE}\n\nbody line\n```"));

    let output = repo.kommit(&[]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{COMMIT_MESSAGE}\n\nbody line\n")
    );
}

#[test]
fn format_warning_goes_to_stderr_without_colors() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    repo.ollama.respond_with("add a feature");

    let output = repo.kommit(&[]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "add a feature\n");
    assert!(
        stderr.contains("does not follow Conventional Commits"),
        "stderr was {stderr}"
    );
    assert!(!stderr.contains('\x1b'), "stderr was {stderr:?}");
}

#[test]
fn commit_keeps_git_output_off_stdout() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");

    let output = repo.kommit(&["--commit"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, format!("{COMMIT_MESSAGE}\n").as_bytes());
    // git commit reports the new commit as "[<branch> <hash>] <subject>".
    assert!(
        stderr.contains(&format!("] {COMMIT_MESSAGE}")),
        "stderr was {stderr}"
    );
}

/// A large diff sends one chat request per chunk plus one for the message, but the model
/// must be loaded once at most, in both modes.
#[test]
fn lm_studio_loads_an_unloaded_model_once_per_run() {
    for stream in ["false", "true"] {
        let repo = TestRepo::with_commit();
        repo.make_large_diff();
        let lm_studio = FakeLmStudio::start(false);

        let output = repo.kommit_with_lm_studio(
            &lm_studio,
            &["--model", LM_STUDIO_MODEL, "--stream", stream],
        );

        assert!(output.status.success(), "stream {stream}: {output:?}");
        assert_eq!(output.stdout, format!("{COMMIT_MESSAGE}\n").as_bytes());
        assert_eq!(lm_studio.chat_requests(), 3, "stream {stream}");
        assert_eq!(lm_studio.load_requests(), 1, "stream {stream}");
    }
}

#[test]
fn lm_studio_reuses_an_already_loaded_model() {
    for stream in ["false", "true"] {
        let repo = TestRepo::with_commit();
        repo.make_large_diff();
        let lm_studio = FakeLmStudio::start(true);

        let output = repo.kommit_with_lm_studio(
            &lm_studio,
            &["--model", LM_STUDIO_MODEL, "--stream", stream],
        );

        assert!(output.status.success(), "stream {stream}: {output:?}");
        assert_eq!(lm_studio.chat_requests(), 3, "stream {stream}");
        assert_eq!(lm_studio.load_requests(), 0, "stream {stream}");
    }
}

#[test]
fn lm_studio_context_overflow_reports_the_cause() {
    for stream in ["false", "true"] {
        let repo = TestRepo::with_commit();
        repo.write("tracked.txt", "changed\n");
        let lm_studio = FakeLmStudio::start_overflowing();

        let output = repo.kommit_with_lm_studio(
            &lm_studio,
            &["--model", LM_STUDIO_MODEL, "--stream", stream],
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "stream {stream}: {output:?}");
        let cause = if stream == "true" {
            "--context-length"
        } else {
            CONTEXT_OVERFLOW_ERROR
        };
        assert!(
            stderr.contains(cause),
            "stream {stream}: stderr was {stderr}"
        );
        assert!(
            !stderr.contains("Is it running?"),
            "stream {stream}: stderr was {stderr}"
        );
        assert_eq!(lm_studio.chat_requests(), 1, "stream {stream}");
    }
}

#[test]
fn lm_studio_unreachable_server_asks_whether_it_is_running() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");
    // A port nobody listens on, once the listener is dropped.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let output = repo.kommit_with_lm_studio_port(port, &["--model", LM_STUDIO_MODEL]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        stderr.contains("Failed to connect to LM Studio. Is it running?"),
        "stderr was {stderr}"
    );
}

#[test]
fn lm_studio_unknown_model_stops_before_the_llm() {
    for stream in ["false", "true"] {
        let repo = TestRepo::with_commit();
        repo.write("tracked.txt", "changed\n");
        let lm_studio = FakeLmStudio::start(false);

        let output = repo.kommit_with_lm_studio(
            &lm_studio,
            &["--model", "missing-model", "--stream", stream],
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "stream {stream}: {output:?}");
        assert!(
            stderr.contains("Model 'missing-model' was not found in LM Studio")
                && stderr.contains("--pull"),
            "stream {stream}: stderr was {stderr}"
        );
        assert_eq!(lm_studio.chat_requests(), 0, "stream {stream}");
        assert_eq!(lm_studio.load_requests(), 0, "stream {stream}");
        assert_eq!(lm_studio.download_requests(), 0, "stream {stream}");
    }
}

#[test]
fn ollama_unknown_model_stops_before_the_llm() {
    for stream in ["false", "true"] {
        let repo = TestRepo::with_commit();
        repo.write("tracked.txt", "changed\n");

        let output = repo.kommit(&["--model", MISSING_MODEL, "--stream", stream]);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "stream {stream}: {output:?}");
        assert!(
            stderr.contains("Model 'missing-model' was not found in Ollama")
                && stderr.contains("--pull")
                && stderr.contains("ollama pull missing-model"),
            "stream {stream}: stderr was {stderr}"
        );
        assert!(output.stdout.is_empty(), "stream {stream}: {output:?}");
        assert!(repo.ollama.prompts().is_empty(), "stream {stream}");
        assert_eq!(repo.ollama.pull_requests(), 0, "stream {stream}");
    }
}

/// A large diff sends one generate request per chunk plus one for the message, but the
/// model must be looked up once at most, in both modes.
#[test]
fn ollama_checks_the_model_once_per_run() {
    for stream in ["false", "true"] {
        let repo = TestRepo::with_commit();
        repo.make_large_diff();

        let output = repo.kommit(&["--stream", stream]);

        assert!(output.status.success(), "stream {stream}: {output:?}");
        assert_eq!(output.stdout, format!("{COMMIT_MESSAGE}\n").as_bytes());
        assert_eq!(repo.ollama.prompts().len(), 3, "stream {stream}");
        assert_eq!(repo.ollama.show_requests(), 1, "stream {stream}");
    }
}

/// A large diff sends several generate requests, but the model is pulled once, before any
/// of them, and its progress stays off stdout.
#[test]
fn ollama_pull_downloads_a_missing_model_once_before_generating() {
    for stream in ["false", "true"] {
        let repo = TestRepo::with_commit();
        repo.make_large_diff();

        let output = repo.kommit(&["--model", MISSING_MODEL, "--pull", "--stream", stream]);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "stream {stream}: {output:?}");
        assert_eq!(output.stdout, format!("{COMMIT_MESSAGE}\n").as_bytes());
        assert!(
            stderr.contains("pulling manifest") && stderr.contains("Pulled model 'missing-model'"),
            "stream {stream}: {stderr}"
        );
        assert_eq!(repo.ollama.pull_requests(), 1, "stream {stream}");
        assert_eq!(repo.ollama.prompts().len(), 3, "stream {stream}");
    }
}

#[test]
fn ollama_pull_skips_an_installed_model() {
    let repo = TestRepo::with_commit();
    repo.write("tracked.txt", "changed\n");

    repo.run_ok(&["--pull"]);

    assert_eq!(repo.ollama.pull_requests(), 0);
}

#[test]
fn ollama_pull_does_nothing_without_changes() {
    let repo = TestRepo::with_commit();

    let output = repo.kommit(&["--model", MISSING_MODEL, "--pull"]);

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(repo.ollama.pull_requests(), 0);
}

#[test]
fn lm_studio_pull_downloads_and_loads_a_missing_model_once() {
    for stream in ["false", "true"] {
        let repo = TestRepo::with_commit();
        repo.make_large_diff();
        let lm_studio = FakeLmStudio::start_without_model();

        let output = repo.kommit_with_lm_studio(
            &lm_studio,
            &["--model", LM_STUDIO_MODEL, "--pull", "--stream", stream],
        );

        assert!(output.status.success(), "stream {stream}: {output:?}");
        assert_eq!(output.stdout, format!("{COMMIT_MESSAGE}\n").as_bytes());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("Downloaded model '{LM_STUDIO_MODEL}'")),
            "stream {stream}: {stderr}"
        );
        assert_eq!(lm_studio.download_requests(), 1, "stream {stream}");
        assert_eq!(lm_studio.load_requests(), 1, "stream {stream}");
        assert_eq!(lm_studio.chat_requests(), 3, "stream {stream}");
    }
}
