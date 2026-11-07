//! `agent-office dispatch --submit` as Hermes runs it: a separate process,
//! one stdout line, and no `exec` into a host. In-crate tests stop at the
//! argument vectors and at `prepare_submit`; they never cross `process::exit`
//! or `Command::spawn`. The binaries here are scripts this file writes.
//! A lookup that would select anything else refuses to spawn.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const TASK: &str = "写接口";
const FAST: Duration = Duration::from_secs(5);
const DSH_OK: Duration = Duration::from_secs(12);
const DSH_ABSENT: Duration = Duration::from_secs(15);

const CLAUDE_STUB: &str = r#"#!/bin/sh
echo invoked >> "$STUB_COUNT"
printf '%s\0' "$@" >> "$STUB_ARGS"
prev=
for arg in "$@"; do
    if [ "$prev" = "--session-id" ]; then
        printf '%s\n' "$arg"
        exit 0
    fi
    prev=$arg
done
exit 2
"#;

const CURSOR_STUB: &str = r#"#!/bin/sh
echo invoked >> "$STUB_COUNT"
printf '%s\0' "$@" >> "$STUB_ARGS"
exit 0
"#;

const SENTINEL: &str = r#"#!/bin/sh
echo invoked >> "$STUB_COUNT"
printf '%s\0' "$@" >> "$STUB_ARGS"
exit 99
"#;

struct Launch {
    root: PathBuf,
    socket: PathBuf,
    home: PathBuf,
    path: PathBuf,
    dsh_home: PathBuf,
    cursor_config: PathBuf,
    claude_config: PathBuf,
    work: PathBuf,
    stub_args: PathBuf,
    stub_count: PathBuf,
}

struct Finished {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Launch {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("agent-office-submit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let launch = Self {
            socket: root.join("office.sock"),
            home: root.join("home"),
            path: root.join("bin"),
            dsh_home: root.join("dsh"),
            cursor_config: root.join("cursor-config"),
            claude_config: root.join("claude-config"),
            work: root.join("repo"),
            stub_args: root.join("args"),
            stub_count: root.join("count"),
            root,
        };
        for dir in [
            &launch.home,
            &launch.path,
            &launch.dsh_home,
            &launch.cursor_config,
            &launch.claude_config,
            &launch.work,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        launch
    }

    fn run(&self, args: &[String], timeout: Duration) -> Finished {
        let bin = office_bin();
        let mut child = Command::new(&bin)
            .args(args)
            .current_dir(&self.work)
            .env("AGENT_OFFICE_LANG", "zh")
            .env("AGENT_OFFICE_SOCKET", &self.socket)
            .env("HOME", &self.home)
            .env("PATH", &self.path)
            .env("DSH_HOME", &self.dsh_home)
            .env("CURSOR_CONFIG_DIR", &self.cursor_config)
            .env("CLAUDE_CONFIG_DIR", &self.claude_config)
            .env("STUB_ARGS", &self.stub_args)
            .env("STUB_COUNT", &self.stub_count)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", bin.display()));
        let status = wait_child(&mut child, timeout);
        let mut stdout = String::new();
        let mut stderr = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .unwrap();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        Finished {
            status,
            stdout,
            stderr,
        }
    }

    fn ledger(&self) -> PathBuf {
        self.root.join("dispatched.jsonl")
    }

    fn invocations(&self) -> usize {
        std::fs::read_to_string(&self.stub_count)
            .unwrap_or_default()
            .lines()
            .filter(|line| *line == "invoked")
            .count()
    }
}

impl Drop for Launch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn office_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_agent_office") {
        let path = PathBuf::from(path);
        assert!(path.is_file(), "missing {}", path.display());
        return path;
    }
    let exe = std::env::current_exe().unwrap();
    let candidate = exe.parent().unwrap().parent().unwrap().join("agent-office");
    assert!(
        candidate.is_file(),
        "no CARGO_BIN_EXE_agent_office and {} is missing (test exe {})",
        candidate.display(),
        exe.display()
    );
    candidate
}

fn wait_child(child: &mut std::process::Child, timeout: Duration) -> ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            panic!("dispatch did not exit within {timeout:?}; killed the child");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

fn install_sentinel(launch: &Launch) {
    for name in ["claude", "cursor-agent"] {
        write_exe(&launch.path.join(name), SENTINEL);
    }
}

/// `connect cursor` / `connect dsh` into this launch's config, so a later
/// `--submit` sees the same files `installed()` reads.
fn connect(launch: &Launch, which: &str) {
    let finished = launch.run(&["connect".into(), which.into()], FAST);
    assert!(
        finished.status.success(),
        "connect {which} failed\nstdout {}\nstderr {}",
        finished.stdout,
        finished.stderr
    );
}

fn bridge_request_present(dsh_home: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dsh_home.join("agent-office/dispatch")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.ends_with(".json")
            && !name.contains(".result.")
            && !name.contains(".claim.")
            && !name.ends_with(".tmp")
    })
}

fn write_hooks(dir: &Path) {
    let events = [
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "Stop",
        "PreToolUse",
        "PostToolUse",
        "Notification",
        "PermissionRequest",
        "SubagentStart",
        "SubagentStop",
    ];
    let mut hooks = serde_json::Map::new();
    for event in events {
        hooks.insert(
            event.to_owned(),
            serde_json::json!([
                {
                    "matcher": "",
                    "hooks": [{
                        "type": "command",
                        "command": "agent-office hook",
                        "timeout": 5
                    }]
                }
            ]),
        );
    }
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_vec(&serde_json::json!({ "hooks": hooks })).unwrap(),
    )
    .unwrap();
}

fn submit_cmd(agent: &str, dir: &Path) -> Vec<String> {
    vec![
        "dispatch".into(),
        "--submit".into(),
        "--agent".into(),
        agent.into(),
        "--role".into(),
        "backend".into(),
        "--dir".into(),
        dir.display().to_string(),
        "--".into(),
        TASK.into(),
    ]
}

fn listen(launch: &Launch) -> std::os::unix::net::UnixListener {
    std::os::unix::net::UnixListener::bind(&launch.socket).unwrap()
}

fn one_json(stdout: &str) -> serde_json::Value {
    let lines: Vec<_> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "stdout should be one JSON line, got {stdout:?}"
    );
    serde_json::from_str(lines[0]).unwrap_or_else(|err| panic!("{err}: {}", lines[0]))
}

fn assert_ok(value: &serde_json::Value, host: &str, dir: &Path) -> String {
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["host"], host);
    assert_eq!(value["role"], "backend");
    assert_eq!(
        value["dir"],
        dir.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(value["text"], "已开工");
    assert!(value.get("error").is_none(), "{value}");
    value["session_id"].as_str().unwrap().to_owned()
}

fn assert_fail(value: &serde_json::Value, host: &str) -> String {
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["host"], host);
    assert_eq!(value["role"], "backend");
    assert!(value.get("session_id").is_none(), "{value}");
    assert!(value.get("text").is_none(), "{value}");
    value["error"].as_str().unwrap().to_owned()
}

fn args_of(path: &Path) -> Vec<String> {
    let raw = std::fs::read(path).unwrap_or_default();
    raw.split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8(part.to_vec()).unwrap())
        .collect()
}

fn is_uuid_v4(id: &str) -> bool {
    let bytes = id.as_bytes();
    let hyphens = [8usize, 13, 18, 23];
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            hyphens.contains(&index) && *byte == b'-'
                || !hyphens.contains(&index) && byte.is_ascii_hexdigit()
        })
        && bytes[14] == b'4'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

fn has_pair(args: &[String], left: &str, right: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == left && pair[1] == right)
}

fn claude_hits(home: &Path, path: &Path) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    for candidate in [
        path.join("claude"),
        home.join(".claude/local/claude"),
        home.join(".local/bin/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ] {
        if candidate.is_file() {
            hits.push(candidate);
        }
    }
    for root in [
        home.join(".vscode/extensions"),
        home.join(".cursor/extensions"),
    ] {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with("anthropic.claude-code-") {
                continue;
            }
            let bin = entry.path().join("resources/native-binary/claude");
            if bin.is_file() {
                hits.push(bin);
            }
        }
    }
    hits
}

fn cursor_hits(home: &Path, path: &Path) -> Vec<PathBuf> {
    [
        path.join("cursor-agent"),
        home.join(".local/bin/cursor-agent"),
        PathBuf::from("/opt/homebrew/bin/cursor-agent"),
        PathBuf::from("/usr/local/bin/cursor-agent"),
    ]
    .into_iter()
    .filter(|candidate| candidate.is_file())
    .collect()
}

fn assert_ledger(launch: &Launch, host: &str, session_id: &str, dir: &Path) {
    let text = std::fs::read_to_string(launch.ledger()).unwrap();
    let lines: Vec<_> = text.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 1, "{text}");
    let row: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(row["session_id"], session_id);
    assert_eq!(row["host"], host);
    assert_eq!(row["role"], "backend");
    assert_eq!(
        row["dir"],
        dir.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(row["task"], TASK);
}

/// A parse error is not a receipt. It must not reach a host: `--submit` with
/// nothing to do, or combined with `--terminal`, would either start an empty
/// session or `exec` over Hermes.
#[test]
fn submit_parse_errors_stay_on_stderr_and_do_not_start_a_host() {
    let launch = Launch::new("parse");
    install_sentinel(&launch);
    let empty = launch.run(
        &[
            "dispatch".into(),
            "--submit".into(),
            "--role".into(),
            "backend".into(),
        ],
        FAST,
    );
    assert!(!empty.status.success(), "{}", empty.stderr);
    assert!(
        empty.stderr.contains("--submit 要带任务"),
        "{}",
        empty.stderr
    );
    assert!(
        !empty.stdout.contains("\"ok\""),
        "a parse error is not a receipt: {}",
        empty.stdout
    );

    let both = launch.run(
        &[
            "dispatch".into(),
            "--submit".into(),
            "--terminal".into(),
            "--role".into(),
            "backend".into(),
            "--".into(),
            TASK.into(),
        ],
        FAST,
    );
    assert!(!both.status.success(), "{}", both.stderr);
    assert!(
        both.stderr.contains("--submit 和 --terminal 不能一起用"),
        "{}",
        both.stderr
    );
    assert!(!both.stdout.contains("\"ok\""), "{}", both.stdout);
    assert_eq!(launch.invocations(), 0, "a parse error started a host");
}

/// The directory is checked before any host. A missing one is still a receipt
/// Hermes can branch on, not a sentence on stderr and not an exec.
#[test]
fn a_missing_directory_is_one_stdout_json_line_and_a_nonzero_exit() {
    let launch = Launch::new("missing-dir");
    install_sentinel(&launch);
    let gone = launch.root.join("gone");
    let cmd = submit_cmd("claude", &gone);
    let finished = launch.run(&cmd, FAST);
    assert!(!finished.status.success(), "{}", finished.stdout);
    let value = one_json(&finished.stdout);
    let error = assert_fail(&value, "claude");
    assert_eq!(value["dir"], gone.display().to_string());
    assert!(
        error.contains("没有") && error.contains(&gone.display().to_string()),
        "{error}"
    );
    assert_eq!(launch.invocations(), 0);
    assert!(!launch.ledger().exists());
}

/// The office has to be listening before Claude, Cursor or dsh starts. The
/// sentence is the one Hermes was told to show, and the sentinel on PATH is
/// what would have run if the check were skipped.
#[test]
fn a_down_office_socket_fails_before_any_host_with_the_fixed_sentence() {
    let launch = Launch::new("office-down");
    install_sentinel(&launch);
    assert!(
        !launch.socket.exists(),
        "the socket must be absent for this case"
    );
    let cmd = submit_cmd("claude", &launch.work);
    let finished = launch.run(&cmd, FAST);
    assert!(!finished.status.success(), "{}", finished.stdout);
    let value = one_json(&finished.stdout);
    let error = assert_fail(&value, "claude");
    assert_eq!(
        error,
        "agent-office 没在运行，派不了：先开着它，进度才能送出来"
    );
    assert_eq!(
        launch.invocations(),
        0,
        "a host ran: {}",
        String::from_utf8_lossy(&std::fs::read(&launch.stub_args).unwrap_or_default())
    );
    assert!(!launch.ledger().exists());
}

/// `claude` missing is a receipt, not a panic and not a search that falls
/// through to a real binary. The spawn is refused unless every place
/// `claude_bin` looks is empty.
#[test]
fn a_missing_claude_binary_is_stdout_json_and_does_not_spawn() {
    let launch = Launch::new("no-claude");
    let hits = claude_hits(&launch.home, &launch.path);
    assert!(
        hits.is_empty(),
        "refusing to spawn; a claude binary would be selected: {hits:?}"
    );
    let _listener = listen(&launch);
    let cmd = submit_cmd("claude", &launch.work);
    let finished = launch.run(&cmd, FAST);
    assert!(!finished.status.success(), "{:?}", finished.stdout);
    let value = one_json(&finished.stdout);
    let error = assert_fail(&value, "claude");
    assert_eq!(error, "找不到 claude 命令：装好 Claude Code");
    assert_eq!(launch.invocations(), 0);
    assert!(!launch.ledger().exists());
}

/// Same for `cursor-agent`: the fixed install paths are part of the lookup,
/// so this refuses to run unless those paths are empty too.
#[test]
fn a_missing_cursor_binary_is_stdout_json_and_does_not_spawn() {
    let launch = Launch::new("no-cursor");
    let hits = cursor_hits(&launch.home, &launch.path);
    assert!(
        hits.is_empty(),
        "refusing to spawn; cursor-agent would be selected: {hits:?}"
    );
    let _listener = listen(&launch);
    let cmd = submit_cmd("cursor", &launch.work);
    let finished = launch.run(&cmd, FAST);
    assert!(!finished.status.success(), "{:?}", finished.stdout);
    let value = one_json(&finished.stdout);
    let error = assert_fail(&value, "cursor");
    assert_eq!(error, "找不到 cursor-agent 命令：先装好 Cursor CLI");
    assert_eq!(launch.invocations(), 0);
    assert!(!launch.ledger().exists());
}

/// The process Hermes started has to be the one that returns. The stub records
/// the argv and exits; an `exec` would make that stub the process stdout
/// instead of this receipt. Hooks are a stand-in file, not a live Claude.
#[test]
fn claude_submit_spawns_the_planned_args_and_prints_a_receipt() {
    let launch = Launch::new("claude-ok");
    let stub = launch.path.join("claude");
    write_exe(&stub, CLAUDE_STUB);
    let hits = claude_hits(&launch.home, &launch.path);
    assert_eq!(
        hits.first(),
        Some(&stub),
        "refusing to spawn; first claude hit is not the stub: {hits:?}"
    );
    write_hooks(&launch.claude_config);
    let _listener = listen(&launch);
    let cmd = submit_cmd("claude", &launch.work);
    let finished = launch.run(&cmd, FAST);
    assert!(
        finished.status.success(),
        "stdout {}\nstderr {}",
        finished.stdout,
        finished.stderr
    );
    assert_eq!(launch.invocations(), 1, "claude was not spawned once");
    let value = one_json(&finished.stdout);
    let session_id = assert_ok(&value, "claude", &launch.work);
    assert!(is_uuid_v4(&session_id), "{session_id}");
    let args = args_of(&launch.stub_args);
    assert!(
        args.iter().any(|arg| arg == "--bg"),
        "missing --bg: {args:?}"
    );
    assert!(has_pair(&args, "--session-id", &session_id), "{args:?}");
    assert!(has_pair(&args, "--agent", "office-backend"), "{args:?}");
    assert!(
        has_pair(&args, "--permission-mode", "bypassPermissions"),
        "{args:?}"
    );
    assert!(!args.iter().any(|arg| arg == "--bare"), "{args:?}");
    assert!(
        args.iter().all(|arg| !arg.contains("claude://")),
        "{args:?}"
    );
    assert!(
        args.iter().any(|arg| arg.contains(TASK)),
        "the task never reached claude: {args:?}"
    );
    assert_ledger(&launch, "claude", &session_id, &launch.work);
}

/// Cursor's argv is what `cursor-agent` would be given. The stub exits 0 at
/// once and must still be a receipt: that is the launch this command treats
/// as started. It must not be a `cursor://` link.
#[test]
fn cursor_submit_spawns_the_planned_args_and_prints_a_receipt() {
    let launch = Launch::new("cursor-ok");
    let stub = launch.path.join("cursor-agent");
    write_exe(&stub, CURSOR_STUB);
    let hits = cursor_hits(&launch.home, &launch.path);
    assert_eq!(
        hits.first(),
        Some(&stub),
        "refusing to spawn; first cursor-agent hit is not the stub: {hits:?}"
    );
    connect(&launch, "cursor");
    let _listener = listen(&launch);
    let cmd = submit_cmd("cursor", &launch.work);
    let finished = launch.run(&cmd, FAST);
    assert!(
        finished.status.success(),
        "stdout {}\nstderr {}",
        finished.stdout,
        finished.stderr
    );
    assert_eq!(launch.invocations(), 1, "cursor-agent was not spawned once");
    let value = one_json(&finished.stdout);
    let session_id = assert_ok(&value, "cursor", &launch.work);
    assert!(is_uuid_v4(&session_id), "{session_id}");
    let args = args_of(&launch.stub_args);
    for flag in ["--print", "--trust", "--force", "--new-session-id"] {
        assert!(
            args.iter().any(|arg| arg == flag),
            "missing {flag}: {args:?}"
        );
    }
    assert!(has_pair(&args, "--new-session-id", &session_id), "{args:?}");
    assert!(
        args.iter().all(|arg| !arg.contains("cursor://")),
        "{args:?}"
    );
    assert!(
        args.iter().any(|arg| arg.contains(TASK)),
        "the task never reached cursor-agent: {args:?}"
    );
    assert_ledger(&launch, "cursor", &session_id, &launch.work);
}

/// The office cannot see a Cursor session until `connect cursor` is done.
/// A receipt of「已开工」would leave Telegram waiting for progress that never
/// arrives, so `--submit` must fail before the stand-in starts and must not
/// write the ledger.
#[test]
fn cursor_submit_does_not_start_the_stand_in_when_events_are_not_connected() {
    let launch = Launch::new("cursor-unwired");
    let stub = launch.path.join("cursor-agent");
    write_exe(&stub, CURSOR_STUB);
    let hits = cursor_hits(&launch.home, &launch.path);
    assert_eq!(
        hits.first(),
        Some(&stub),
        "refusing to run; cursor-agent would not be the stub: {hits:?}"
    );
    assert!(
        !launch.cursor_config.join("hooks.json").exists(),
        "this case is the one where connect cursor has not been done"
    );
    let _listener = listen(&launch);
    let cmd = submit_cmd("cursor", &launch.work);
    let finished = launch.run(&cmd, FAST);
    assert!(!finished.status.success(), "{}", finished.stdout);
    let value = one_json(&finished.stdout);
    let error = assert_fail(&value, "cursor");
    assert_eq!(
        error,
        "Cursor 事件没接上，进度送不出来：先运行 agent-office connect cursor"
    );
    assert_eq!(
        launch.invocations(),
        0,
        "the stand-in started: {}",
        String::from_utf8_lossy(&std::fs::read(&launch.stub_args).unwrap_or_default())
    );
    assert!(!launch.ledger().exists(), "a refused submit wrote the ledger");
    let logs: Vec<_> = std::fs::read_dir(&launch.root)
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("cursor-") && name.ends_with(".log"))
        })
        .map(|entry| entry.path())
        .collect();
    assert!(
        logs.is_empty(),
        "opened a cursor log before refusing: {logs:?}"
    );
}

/// dsh `--submit` is the file bridge with `submit: true`. A sentinel named
/// `claude` and `cursor-agent` is on PATH so a mistaken exec is visible.
/// `focus_app` is not observable from here; this checks the request the
/// bridge actually published.
#[test]
fn dsh_submit_uses_the_bridge_and_prints_a_receipt() {
    let launch = Launch::new("dsh-ok");
    install_sentinel(&launch);
    connect(&launch, "dsh");
    let _listener = listen(&launch);
    let (tx, rx) = mpsc::channel();
    let dsh_home = launch.dsh_home.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let watcher_stop = Arc::clone(&stop);
    let watcher = std::thread::spawn(move || {
        if let Some(request) = answer_dsh(&dsh_home, &watcher_stop) {
            let _ = tx.send(request);
        }
    });
    let cmd = submit_cmd("dsh", &launch.work);
    let finished = launch.run(&cmd, DSH_OK);
    stop.store(true, Ordering::Relaxed);
    watcher.join().unwrap();
    assert!(
        finished.status.success(),
        "stdout {}\nstderr {}",
        finished.stdout,
        finished.stderr
    );
    assert_eq!(launch.invocations(), 0, "dsh submit started another host");
    let request = rx.try_recv().expect("the bridge never published a request");
    assert_eq!(request["submit"], true, "{request}");
    assert_eq!(request["role"], "backend");
    assert_eq!(
        request["cwd"],
        launch.work.canonicalize().unwrap().display().to_string()
    );
    let prompt = request["prompt"].as_str().unwrap();
    assert!(prompt.contains("[office:backend]"), "{prompt}");
    assert!(prompt.contains(TASK), "{prompt}");
    assert!(!prompt.contains("claude://") && !prompt.contains("cursor://"));
    let value = one_json(&finished.stdout);
    let session_id = assert_ok(&value, "dsh", &launch.work);
    assert_eq!(session_id, "session-dsh-qa");
    assert_ledger(&launch, "dsh", &session_id, &launch.work);
}

/// No Host answering the bridge is a receipt with a non-zero exit, not a hang
/// and not an `open` of the app. The wait is the bridge's own ten seconds.
#[test]
fn a_missing_dsh_host_is_stdout_json_and_a_nonzero_exit() {
    let launch = Launch::new("dsh-down");
    install_sentinel(&launch);
    // The plugin is connected. A Host that never answers still fails inside
    // the bridge; that path is not the "events are unwired" refusal.
    connect(&launch, "dsh");
    let _listener = listen(&launch);
    let cmd = submit_cmd("dsh", &launch.work);
    let finished = launch.run(&cmd, DSH_ABSENT);
    assert!(!finished.status.success(), "{}", finished.stdout);
    let value = one_json(&finished.stdout);
    let error = assert_fail(&value, "dsh");
    assert!(error.contains("DSH") && error.contains("超时"), "{error}");
    assert_eq!(
        launch.invocations(),
        0,
        "a missing DSH host started another host"
    );
    assert!(!launch.ledger().exists());
}

/// The office cannot see a DSH session until `connect dsh` mounts the plugin.
/// A receipt of「已开工」would leave Telegram waiting for progress that never
/// arrives, so `--submit` must fail before the bridge and must not write the
/// ledger. A missing Host is a different failure and still uses the bridge.
#[test]
fn dsh_submit_does_not_call_the_bridge_when_the_plugin_is_not_connected() {
    let launch = Launch::new("dsh-unwired");
    install_sentinel(&launch);
    let _listener = listen(&launch);
    let (tx, rx) = mpsc::channel();
    let dsh_home = launch.dsh_home.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let watcher_stop = Arc::clone(&stop);
    let watcher = std::thread::spawn(move || {
        let started = Instant::now();
        while !watcher_stop.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(8) {
            if bridge_request_present(&dsh_home) {
                let _ = tx.send(());
                return;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    });
    let cmd = submit_cmd("dsh", &launch.work);
    let started = Instant::now();
    let finished = launch.run(&cmd, FAST);
    stop.store(true, Ordering::Relaxed);
    watcher.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "returned in {:?}; calling the bridge waits 10s",
        started.elapsed()
    );
    assert!(
        rx.try_recv().is_err(),
        "the bridge published a dispatch request"
    );
    assert!(!bridge_request_present(&launch.dsh_home));
    assert!(!finished.status.success(), "{}", finished.stdout);
    let value = one_json(&finished.stdout);
    let error = assert_fail(&value, "dsh");
    assert_eq!(
        error,
        "DSH 事件插件没接好，进度送不出来：先运行 agent-office connect dsh"
    );
    assert_eq!(launch.invocations(), 0, "dsh submit started another host");
    assert!(!launch.ledger().exists(), "a refused submit wrote the ledger");
}

fn answer_dsh(dsh_home: &Path, stop: &AtomicBool) -> Option<serde_json::Value> {
    let dispatch = dsh_home.join("agent-office/dispatch");
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(8) {
        let Ok(entries) = std::fs::read_dir(&dispatch) else {
            std::thread::sleep(Duration::from_millis(15));
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json")
                || name.contains(".result.")
                || name.contains(".claim.")
                || name.ends_with(".tmp")
            {
                continue;
            }
            let raw = std::fs::read(entry.path()).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            let id = value["id"].as_str().unwrap();
            let result = serde_json::json!({
                "id": id,
                "ok": true,
                "sessionId": "session-dsh-qa",
                "agentPreset": "office-backend",
            });
            let tmp = dispatch.join(format!("{id}.result.json.tmp"));
            std::fs::write(&tmp, serde_json::to_vec(&result).unwrap()).unwrap();
            std::fs::rename(tmp, dispatch.join(format!("{id}.result.json"))).unwrap();
            return Some(value);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    None
}
