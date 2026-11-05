//! What a Claude Code transcript says the agent was asked to do. pixtuoid's
//! decoder keeps only tool calls, so the prompts and todos come from here.
//! agent-office's dsh plugin writes its task logs in the same shape, with
//! dsh's own tool names (`bash`, `read`, `todo_write`, ...), and a Codex
//! rollout says the same in its own lines (`event_msg` / `response_item`).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::text::{one_line, truncate_cells};

const KEEP_PROMPTS: usize = 3;
/// How much of a transcript the first read takes from its end: sessions run
/// to 100 MB, nearly all of it tool output.
const FIRST_READ: u64 = 4 << 20;
const KEEP_TOOLS: usize = 5;
/// How much of the agent's last reply is kept, in characters.
const KEEP_REPLY: usize = 3000;

#[derive(Debug, Default, Clone)]
pub struct Transcript {
    /// Oldest first; the last is what the agent works on now.
    pub prompts: VecDeque<String>,
    /// The in-progress todo's `activeForm` (or content).
    pub todo_active: Option<String>,
    /// Oldest first, each `Tool target`.
    pub recent_tools: VecDeque<String>,
    pub tool_calls: u32,
    /// What kind of work the last tool call was.
    pub phase: Option<Phase>,
    /// Tool results in a row that came back as errors, since the human last
    /// spoke; a call the user turned down is not one.
    pub fail_streak: u32,
    /// Each call whose result is not in yet, as `(tool_use id, tool)`.
    pub outstanding: Vec<(String, String)>,
    /// When the human last spoke, which is when the current turn began;
    /// `None` when that line carried no timestamp.
    pub turn_started: Option<SystemTime>,
    /// The agent's last words to the human since they last spoke: once the
    /// turn is over, its answer, its report, or its question.
    pub last_reply: Option<String>,
    /// Calls out that earn a cheer if they succeed, as `(tool_use id, cheer)`.
    pub(super) cheering: Vec<(String, Cheer)>,
    /// The last thing worth cheering and when its result came in.
    pub cheer: Option<(Cheer, SystemTime)>,
}

/// Something the agent just pulled off that the office cheers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cheer {
    /// A test run came back green.
    TestsPassed,
    /// It pushed its work or opened a pull request.
    Shipped,
    /// A call went through after a run of failures.
    Recovered,
}

/// Failures in a row that make the next success a comeback: the run that
/// marks an agent stuck.
const RECOVER_AFTER: u32 = 3;

/// Words in a shell command that mean it runs tests, not lints or builds:
/// only a test run going green is cheered as one.
const TEST_WORDS: &[&str] = &["test", "jest", "spec"];

/// What a call is cheered for if it succeeds.
fn cheer_of(name: &str, input: &Value) -> Option<Cheer> {
    if name.contains("create_pull_request") {
        return Some(Cheer::Shipped);
    }
    let shell = matches!(name, "Bash" | "bash" | "Shell" | "shell") || CODEX_SHELLS.contains(&name);
    if !shell {
        return None;
    }
    let cmd = shell_command(input)?.to_lowercase();
    if cmd.contains("git push") || cmd.contains("gh pr create") {
        Some(Cheer::Shipped)
    } else if TEST_WORDS.iter().any(|w| cmd.contains(w)) {
        Some(Cheer::TestsPassed)
    } else {
        None
    }
}

/// More calls than this out at once is results lost to a mid-file first read.
const KEEP_OUTSTANDING: usize = 32;
/// How Claude Code words the result of a call the user turned down.
const REJECTED: &str = "doesn't want to proceed with this tool use";

impl Transcript {
    /// Whether any call still out is one of `tools`.
    pub fn awaits_any(&self, tools: &[&str]) -> bool {
        self.outstanding
            .iter()
            .any(|(_, t)| tools.contains(&t.as_str()))
    }
}

/// A tool result's text, however its content is shaped.
fn result_text(b: &Value) -> String {
    match b.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// A call the user turned down: Claude Code words its result so, and the dsh
/// plugin flags it, since dsh's wording is not Claude Code's.
fn rejected(b: &Value) -> bool {
    b.get("rejected").and_then(Value::as_bool) == Some(true) || result_text(b).contains(REJECTED)
}

/// What kind of work an agent is at, read off its last tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Research,
    Code,
    Test,
    Run,
    Delegate,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Research => "调研",
            Phase::Code => "写码",
            Phase::Test => "测试",
            Phase::Run => "执行",
            Phase::Delegate => "派活",
        }
    }
}

/// Words in a shell command that mean it checks the work rather than does it.
const CHECK_WORDS: &[&str] = &[
    "test",
    "jest",
    "spec",
    "clippy",
    "cargo check",
    "tsc",
    "lint",
];

fn phase_of(name: &str, input: &Value) -> Option<Phase> {
    Some(match name {
        "Read" | "Grep" | "Glob" | "WebFetch" | "WebSearch" | "LSP" => Phase::Research,
        "read" | "read_image" | "grep" | "glob" | "web_fetch" | "web_search" => Phase::Research,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Phase::Code,
        // Cursor's names.
        "StrReplace" | "Delete" => Phase::Code,
        "LS" => Phase::Research,
        "edit" | "write" => Phase::Code,
        // One tool for looking and changing; `view` only reads.
        "str_replace_editor" if input.get("command").and_then(Value::as_str) == Some("view") => {
            Phase::Research
        }
        "str_replace_editor" => Phase::Code,
        "Task" | "Agent" | "subagent" | "subagent_fork" | "spawn_agent" => Phase::Delegate,
        "apply_patch" => Phase::Code,
        "view_image" => Phase::Research,
        // Cursor's `Shell` too: like Codex, it reads through its shell.
        n if CODEX_SHELLS.contains(&n) || n == "Shell" => {
            let cmd = shell_command(input).unwrap_or_default().to_lowercase();
            let first = cmd.split_whitespace().next().unwrap_or_default();
            if CHECK_WORDS.iter().any(|w| cmd.contains(w)) {
                Phase::Test
            } else if LOOK_COMMANDS.contains(&first) {
                Phase::Research
            } else {
                Phase::Run
            }
        }
        "Bash" | "bash" | "Shell" | "shell" => {
            let cmd = input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase();
            if CHECK_WORDS.iter().any(|w| cmd.contains(w)) {
                Phase::Test
            } else {
                Phase::Run
            }
        }
        _ => return None,
    })
}

impl Transcript {
    pub fn latest_prompt(&self) -> Option<&str> {
        self.prompts.back().map(String::as_str)
    }

    /// Fold one JSONL line in; unparseable lines are skipped, as a partial
    /// write or a format change must not stop the rest.
    pub fn feed(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("user") => self.feed_user(&v),
            Some("assistant") => self.feed_assistant(&v),
            Some("event_msg" | "response_item") => self.feed_codex(&v),
            // Claude Code's own note of the last prompt: what a tail read
            // that starts past the prompt itself still finds.
            Some("last-prompt") => {
                if let Some(p) = v.get("lastPrompt").and_then(Value::as_str) {
                    self.push_prompt(p);
                }
            }
            _ => {}
        }
    }

    fn push_prompt(&mut self, text: &str) {
        let Some(p) = human_prompt(text) else { return };
        if self.prompts.back() == Some(&p) {
            return;
        }
        if self.prompts.len() == KEEP_PROMPTS {
            self.prompts.pop_front();
        }
        self.prompts.push_back(p);
    }

    fn feed_user(&mut self, v: &Value) {
        // isMeta: harness caveats. The other two mark the summary /compact
        // writes as a user turn, which would replace the real request.
        let flag = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
        if flag("isMeta") || flag("isCompactSummary") || flag("isVisibleInTranscriptOnly") {
            return;
        }
        let Some(content) = v.pointer("/message/content") else {
            return;
        };
        let text = match content {
            Value::String(s) => s.clone(),
            // A list carrying a tool_result is the harness answering a tool,
            // not the human; one of text blocks is a prompt with attachments.
            Value::Array(blocks) => {
                let results: Vec<&Value> = blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                    .collect();
                if !results.is_empty() {
                    let streak_before = self.fail_streak;
                    for b in &results {
                        if let Some(id) = b.get("tool_use_id").and_then(Value::as_str) {
                            self.outstanding.retain(|(o, _)| o != id);
                        }
                    }
                    let failed = |b: &&Value| {
                        b.get("is_error").and_then(Value::as_bool) == Some(true) && !rejected(b)
                    };
                    if results.iter().any(failed) {
                        self.fail_streak += 1;
                    } else if results.iter().all(|b| !rejected(b)) {
                        self.fail_streak = 0;
                    }
                    let settled: Vec<(&str, bool)> = results
                        .iter()
                        .filter_map(|b| {
                            let id = b.get("tool_use_id").and_then(Value::as_str)?;
                            Some((id, !failed(b) && !rejected(b)))
                        })
                        .collect();
                    self.cheer_on(&settled, streak_before, line_time(v));
                    return;
                }
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            _ => return,
        };
        if human_prompt(&text).is_some() {
            self.human_spoke(v);
        }
        self.push_prompt(&text);
    }

    /// A new turn: whatever went wrong before, the agent starts over, and
    /// what it said last answered the turn before.
    fn human_spoke(&mut self, v: &Value) {
        self.fail_streak = 0;
        self.last_reply = None;
        // Set even when the words repeat the last prompt: it is a new turn.
        self.turn_started = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_timestamp);
    }

    fn set_reply(&mut self, text: &str) {
        let t = one_line(text);
        if t.is_empty() {
            return;
        }
        // The end of a long report is where it concludes.
        let n = t.chars().count();
        self.last_reply = Some(t.chars().skip(n.saturating_sub(KEEP_REPLY)).collect());
    }

    fn feed_assistant(&mut self, v: &Value) {
        let Some(Value::Array(blocks)) = v.pointer("/message/content") else {
            return;
        };
        let mut said = Vec::new();
        for b in blocks {
            match b.get("type").and_then(Value::as_str) {
                Some("text") => said.extend(b.get("text").and_then(Value::as_str)),
                Some("tool_use") => {
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                    let input = b.get("input").unwrap_or(&Value::Null);
                    self.tool_call(b.get("id").and_then(Value::as_str), name, input);
                }
                _ => {}
            }
        }
        if !said.is_empty() {
            self.set_reply(&said.join(" "));
        }
    }

    /// Calls `settled` as `(id, succeeded)`, with the streak of failures
    /// before them: a comeback outranks whatever the call itself was.
    fn cheer_on(&mut self, settled: &[(&str, bool)], streak_before: u32, at: Option<SystemTime>) {
        let mut won = None;
        for (id, ok) in settled {
            if let Some(i) = self.cheering.iter().position(|(o, _)| o == id) {
                let (_, c) = self.cheering.remove(i);
                if *ok {
                    won = Some(c);
                }
            }
        }
        let comeback = streak_before >= RECOVER_AFTER
            && self.fail_streak == 0
            && settled.iter().any(|(_, ok)| *ok);
        let cheer = if comeback {
            Some(Cheer::Recovered)
        } else {
            won
        };
        if let (Some(c), Some(at)) = (cheer, at) {
            self.cheer = Some((c, at));
        }
    }

    fn tool_call(&mut self, id: Option<&str>, name: &str, input: &Value) {
        self.tool_calls += 1;
        if let (Some(id), Some(c)) = (id, cheer_of(name, input)) {
            if self.cheering.len() == KEEP_OUTSTANDING {
                self.cheering.remove(0);
            }
            self.cheering.push((id.to_owned(), c));
        }
        if let Some(id) = id {
            if self.outstanding.len() == KEEP_OUTSTANDING {
                self.outstanding.remove(0);
            }
            self.outstanding.push((id.to_owned(), name.to_owned()));
        }
        if let Some(p) = phase_of(name, input) {
            self.phase = Some(p);
        }
        match name {
            "TodoWrite" | "todo_write" => self.todo_active = active_todo(input),
            "update_plan" => self.todo_active = active_plan_step(input),
            _ => {}
        }
        if self.recent_tools.len() == KEEP_TOOLS {
            self.recent_tools.pop_front();
        }
        self.recent_tools.push_back(tool_line(name, input));
    }

    /// One rollout line. The human's words are the `user_message` event: the
    /// `message` items with role `user` also carry the context Codex injects
    /// (`<environment_context>`, AGENTS.md).
    fn feed_codex(&mut self, v: &Value) {
        let Some(p) = v.get("payload") else { return };
        let s = |k: &str| p.get(k).and_then(Value::as_str);
        match (v.get("type").and_then(Value::as_str), s("type")) {
            (Some("event_msg"), Some("user_message")) => {
                let Some(text) = s("message") else { return };
                if human_prompt(text).is_some() {
                    self.human_spoke(v);
                }
                self.push_prompt(text);
            }
            (Some("event_msg"), Some("agent_message")) => {
                if let Some(text) = s("message") {
                    self.set_reply(text);
                }
            }
            // The turn's own record of how it ended.
            (Some("event_msg"), Some("task_complete")) => {
                if let Some(text) = s("last_agent_message") {
                    self.set_reply(text);
                }
            }
            (Some("response_item"), Some(kind @ ("function_call" | "custom_tool_call"))) => {
                let name = s("name").unwrap_or("?");
                // A function call's arguments are a JSON string; a custom
                // tool's input (apply_patch's patch) is free text.
                let input = if kind == "function_call" {
                    s("arguments")
                        .and_then(|a| serde_json::from_str(a).ok())
                        .unwrap_or(Value::Null)
                } else {
                    serde_json::json!({ "input": s("input").unwrap_or_default() })
                };
                self.tool_call(s("call_id"), name, &input);
            }
            (Some("response_item"), Some("web_search_call")) => {
                let action = p.get("action").cloned().unwrap_or(Value::Null);
                self.tool_call(None, "web_search", &action);
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                if let Some(id) = s("call_id") {
                    self.outstanding.retain(|(o, _)| o != id);
                }
                let streak_before = self.fail_streak;
                let outcome = codex_outcome(p.get("output").unwrap_or(&Value::Null));
                match outcome {
                    Outcome::Failed => self.fail_streak += 1,
                    Outcome::Rejected => {}
                    Outcome::Ok => self.fail_streak = 0,
                }
                if let Some(id) = s("call_id") {
                    self.cheer_on(&[(id, outcome == Outcome::Ok)], streak_before, line_time(v));
                }
            }
            _ => {}
        }
    }
}

/// How a Codex call came back.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Failed,
    /// The user turned it down: their say, not the agent failing.
    Rejected,
}

/// A Codex call's output is text, or JSON text with the exit code in its
/// `metadata` (older `shell`), or an object with a `success` flag. A command
/// says how it exited in its text (`Exit code: 1`, `Process exited with code 1`).
fn codex_outcome(output: &Value) -> Outcome {
    let parsed;
    let output = match output {
        Value::String(t) => match serde_json::from_str::<Value>(t) {
            Ok(v @ Value::Object(_)) => {
                parsed = v;
                &parsed
            }
            _ => output,
        },
        _ => output,
    };
    if output.get("success").and_then(Value::as_bool) == Some(false) {
        return Outcome::Failed;
    }
    if let Some(code) = output
        .pointer("/metadata/exit_code")
        .and_then(Value::as_i64)
    {
        return if code == 0 {
            Outcome::Ok
        } else {
            Outcome::Failed
        };
    }
    let text = match output {
        Value::String(t) => t.as_str(),
        _ => match output.get("content").or_else(|| output.get("output")) {
            Some(Value::String(t)) => t.as_str(),
            _ => "",
        },
    };
    if text.contains("rejected by user") {
        return Outcome::Rejected;
    }
    let exit = text.lines().take(8).find_map(|l| {
        l.strip_prefix("Exit code: ")
            .or_else(|| l.strip_prefix("Process exited with code "))
            .and_then(|c| c.trim().parse::<i64>().ok())
    });
    match exit {
        Some(c) if c != 0 => Outcome::Failed,
        _ => Outcome::Ok,
    }
}

/// The human's words, or `None` for the wrappers the harness injects as user
/// turns (`<system-reminder>`, `<task-notification>`, slash-command echoes,
/// interrupt notices).
fn human_prompt(text: &str) -> Option<String> {
    let t = text.trim_start();
    if t.is_empty() || t.starts_with('<') || t.starts_with("[Request interrupted") {
        return None;
    }
    Some(one_line(t))
}

/// When a transcript line was written, if it says.
fn line_time(v: &Value) -> Option<SystemTime> {
    v.get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
}

/// An RFC 3339 time as transcripts write it (`2026-11-05T09:30:15.937Z`, or
/// with a `+08:00` offset); `None` for anything else.
fn parse_timestamp(s: &str) -> Option<SystemTime> {
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(r)?;
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut nanos = 0u32;
    if b[i] == b'.' {
        let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let frac = &s[i + 1..i + 1 + digits.min(9)];
        nanos = frac.parse::<u32>().ok()? * 10u32.pow(9 - frac.len() as u32);
        i += 1 + digits;
    }
    let offset = match s.get(i..)? {
        "Z" | "z" => 0,
        tz if tz.len() == 6 && tz.as_bytes()[3] == b':' => {
            let sign = match tz.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            sign * (num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60)
        }
        _ => return None,
    };
    // Days since 1970-01-01 in the proleptic Gregorian calendar (Hinnant's
    // days_from_civil).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset;
    let secs = u64::try_from(secs).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::new(secs, nanos))
}

/// The `in_progress` step of Codex's `update_plan`.
fn active_plan_step(input: &Value) -> Option<String> {
    input
        .get("plan")?
        .as_array()?
        .iter()
        .find(|t| t.get("status").and_then(Value::as_str) == Some("in_progress"))
        .and_then(|t| t.get("step").and_then(Value::as_str))
        .map(one_line)
}

/// A shell call's command line: Claude's and dsh's `command`, Codex's `cmd`,
/// or Codex's argv, whose `bash -lc <script>` is the script.
fn shell_command(input: &Value) -> Option<String> {
    let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_owned);
    if let Some(c) = s("cmd").or_else(|| s("command")) {
        return Some(c);
    }
    let argv: Vec<&str> = input
        .get("command")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    match argv.as_slice() {
        [_, flag, script] if flag.starts_with('-') && flag.ends_with('c') => {
            Some((*script).to_owned())
        }
        _ => Some(argv.join(" ")),
    }
}

/// Codex's shell names; it has no read or search tool of its own.
const CODEX_SHELLS: &[&str] = &["exec_command", "shell", "local_shell", "shell_command"];

/// Commands that only look: how Codex reads and searches.
const LOOK_COMMANDS: &[&str] = &[
    "rg", "grep", "cat", "sed", "ls", "find", "head", "tail", "nl", "wc", "fd", "tree",
];

/// The file an `apply_patch` patch touches first.
fn patched_file(patch: &str) -> Option<&str> {
    patch.lines().find_map(|l| {
        ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
            .iter()
            .find_map(|p| l.strip_prefix(p))
            .map(str::trim)
    })
}

fn active_todo(input: &Value) -> Option<String> {
    let todos = input.get("todos")?.as_array()?;
    todos
        .iter()
        .find(|t| t.get("status").and_then(Value::as_str) == Some("in_progress"))
        .and_then(|t| {
            t.get("activeForm")
                .or_else(|| t.get("content"))
                .and_then(Value::as_str)
        })
        .map(one_line)
}

fn tool_line(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    let target = match name {
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "read" | "read_image"
        | "edit" | "write" | "str_replace_editor" | "StrReplace" | "Delete" => s("file_path")
            .or_else(|| s("notebook_path"))
            .or_else(|| s("path"))
            .map(|p| {
                Path::new(p)
                    .file_name()
                    .map_or_else(|| p.to_owned(), |f| f.to_string_lossy().into_owned())
            }),
        // `shell` is Cursor's and Codex's: a command line, or Codex's argv.
        "Bash" | "bash" | "Shell" | "shell" => s("description")
            .map(str::to_owned)
            .or_else(|| shell_command(input)),
        n if CODEX_SHELLS.contains(&n) => shell_command(input),
        "apply_patch" => s("input").and_then(patched_file).map(|p| {
            Path::new(p)
                .file_name()
                .map_or_else(|| p.to_owned(), |f| f.to_string_lossy().into_owned())
        }),
        "spawn_agent" => s("description").or_else(|| s("message")).map(str::to_owned),
        "view_image" => s("path").map(str::to_owned),
        "Grep" | "Glob" | "grep" | "glob" => s("pattern").map(str::to_owned),
        "Task" | "Agent" | "subagent" | "subagent_fork" => s("description").map(str::to_owned),
        "WebFetch" | "web_fetch" => s("url").map(str::to_owned),
        "WebSearch" | "web_search" => s("query").map(str::to_owned),
        _ => None,
    };
    // Tool input is the model's text: one line, no control characters.
    match target {
        Some(t) => format!("{} {}", one_line(name), truncate_cells(&one_line(&t), 48)),
        None => one_line(name),
    }
}

/// A subagent's own description of its job, from the `.meta.json` Claude
/// Code writes beside `agent-<id>.jsonl`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubMeta {
    pub description: Option<String>,
    pub agent_type: Option<String>,
}

pub fn sub_meta(transcript: &Path) -> Option<SubMeta> {
    let stem = transcript.file_stem()?.to_str()?;
    if !stem.starts_with("agent-") {
        return None;
    }
    let meta = transcript.with_file_name(format!("{stem}.meta.json"));
    let v: Value = serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
    let get = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(one_line)
            .filter(|s| !s.is_empty())
    };
    Some(SubMeta {
        description: get("description"),
        agent_type: get("agentType"),
    })
}

/// Reads what a transcript gained since the last poll.
#[derive(Debug)]
pub struct Tail {
    pub path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    skip_partial: bool,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: Vec::new(),
            skip_partial: false,
        }
    }

    /// Feed every complete new line into `t`; whether any arrived.
    pub fn poll(&mut self, t: &mut Transcript) -> std::io::Result<bool> {
        let mut f = File::open(&self.path)?;
        let len = f.metadata()?.len();
        if len < self.offset {
            // Rewritten from scratch: start over.
            *t = Transcript::default();
            self.offset = 0;
            self.partial.clear();
        }
        if len == self.offset {
            return Ok(false);
        }
        if self.offset == 0 && len > FIRST_READ {
            self.offset = len - FIRST_READ;
            // Mid-line: drop up to the first newline as a partial.
            self.skip_partial = true;
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut chunk = Vec::new();
        f.take(len - self.offset).read_to_end(&mut chunk)?;
        self.offset = len;
        self.partial.extend_from_slice(&chunk);
        let Some(last_nl) = self.partial.iter().rposition(|&b| b == b'\n') else {
            return Ok(false);
        };
        let rest = self.partial.split_off(last_nl + 1);
        let mut complete = std::mem::replace(&mut self.partial, rest);
        if std::mem::take(&mut self.skip_partial) {
            let first_nl = complete.iter().position(|&b| b == b'\n').unwrap_or(0);
            complete.drain(..=first_nl);
        }
        for line in complete.split(|&b| b == b'\n') {
            if let Ok(s) = std::str::from_utf8(line) {
                t.feed(s);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(content: Value) -> String {
        json!({"type":"user","message":{"role":"user","content":content}}).to_string()
    }

    fn tool(name: &str, input: Value) -> String {
        json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":name,"input":input}]}})
            .to_string()
    }

    /// A dsh session's task log, as agent-office's plugin writes it: the
    /// prompt, todo, phase and failures must read the same as a Claude Code
    /// session's, or a dsh figure shows no task and is never flagged stuck.
    #[test]
    fn a_dsh_task_log_reads_like_a_claude_transcript() {
        let mut t = Transcript::default();
        let feed = |t: &mut Transcript, v: Value| t.feed(&v.to_string());
        feed(
            &mut t,
            json!({"type":"user","message":{"content":"开始微信读书问答PK"}}),
        );
        feed(
            &mut t,
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"c1",
            "name":"todo_write","input":{"todos":[{"content":"打开 app","status":"completed"},
            {"content":"答 10 题","status":"in_progress"}]}}]}}),
        );
        feed(
            &mut t,
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"c2",
            "name":"bash","input":{"description":"Run quiz","command":"python3 quiz.py"}}]}}),
        );
        assert_eq!(t.latest_prompt(), Some("开始微信读书问答PK"));
        assert_eq!(t.todo_active.as_deref(), Some("答 10 题"));
        assert_eq!(t.phase, Some(Phase::Run));
        assert_eq!(
            t.recent_tools.back().map(String::as_str),
            Some("bash Run quiz")
        );
        assert!(t.awaits_any(&["bash"]));
        let result = |id: &str, err: bool, rejected: bool| {
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,
                "is_error":err,"rejected":rejected}]}})
        };
        for _ in 0..3 {
            feed(&mut t, result("c2", true, false));
        }
        assert_eq!(t.fail_streak, 3);
        assert!(!t.awaits_any(&["bash"]));
        // The user turning a call down is not the agent failing.
        feed(&mut t, result("c3", true, true));
        assert_eq!(t.fail_streak, 3);
        feed(
            &mut t,
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"c4",
            "name":"edit","input":{"file_path":"/repo/src/quiz.py"}}]}}),
        );
        assert_eq!(t.phase, Some(Phase::Code));
        assert_eq!(
            t.recent_tools.back().map(String::as_str),
            Some("edit quiz.py")
        );
    }

    fn rollout(outer: &str, payload: Value) -> String {
        json!({"timestamp":"2026-11-05T09:30:15.000Z","type":outer,"payload":payload}).to_string()
    }

    /// A Codex session's rollout must give the same task, todo, phase and
    /// failures a Claude transcript does, or its figure has no task line and
    /// is never flagged stuck.
    #[test]
    fn a_codex_rollout_reads_like_a_claude_transcript() {
        let mut t = Transcript::default();
        // Codex's injected context rides a `user` message item, not the event.
        t.feed(&rollout(
            "response_item",
            json!({"type":"message","role":"user","content":[{"type":"input_text",
                "text":"<environment_context>cwd</environment_context>"}]}),
        ));
        t.feed(&rollout(
            "response_item",
            json!({"type":"message","role":"user","content":[{"type":"input_text",
                "text":"# AGENTS.md instructions for /repo"}]}),
        ));
        assert_eq!(t.latest_prompt(), None);
        t.feed(&rollout(
            "event_msg",
            json!({"type":"user_message","message":"修复登录页的报错","images":[]}),
        ));
        assert_eq!(t.latest_prompt(), Some("修复登录页的报错"));
        assert!(t.turn_started.is_some());
        let call = |name: &str, args: Value, id: &str| {
            rollout(
                "response_item",
                json!({"type":"function_call","name":name,"arguments":args.to_string(),"call_id":id}),
            )
        };
        t.feed(&call(
            "update_plan",
            json!({"plan":[{"step":"读代码","status":"completed"},{"step":"改登录页","status":"in_progress"}]}),
            "p1",
        ));
        assert_eq!(t.todo_active.as_deref(), Some("改登录页"));
        t.feed(&call("exec_command", json!({"cmd":"rg login src"}), "c1"));
        assert_eq!(
            t.phase,
            Some(Phase::Research),
            "Codex reads through its shell"
        );
        assert_eq!(
            t.recent_tools.back().map(String::as_str),
            Some("exec_command rg login src")
        );
        assert!(t.awaits_any(&["exec_command"]));
        t.feed(&rollout(
            "response_item",
            json!({"type":"custom_tool_call","name":"apply_patch","call_id":"c2",
                "input":"*** Begin Patch\n*** Update File: src/login.ts\n@@\n-a\n+b\n*** End Patch"}),
        ));
        assert_eq!(t.phase, Some(Phase::Code));
        assert_eq!(
            t.recent_tools.back().map(String::as_str),
            Some("apply_patch login.ts")
        );
        t.feed(&call(
            "shell",
            json!({"command":["bash","-lc","cargo test --release"]}),
            "c3",
        ));
        assert_eq!(t.phase, Some(Phase::Test));
        assert_eq!(
            t.recent_tools.back().map(String::as_str),
            Some("shell cargo test --release")
        );
        let output = |id: &str, out: Value| {
            rollout(
                "response_item",
                json!({"type":"function_call_output","call_id":id,"output":out}),
            )
        };
        t.feed(&output(
            "c1",
            json!("Chunk ID: a\nWall time: 0.1 seconds\nProcess exited with code 0\nOutput:\nok"),
        ));
        assert_eq!(t.fail_streak, 0);
        assert!(!t.awaits_any(&["exec_command"]));
        t.feed(&output(
            "c3",
            json!(r#"{"output":"error[E0425]","metadata":{"exit_code":101}}"#),
        ));
        t.feed(&output(
            "c4",
            json!("Exit code: 1\nWall time: 2 seconds\nOutput:\nboom"),
        ));
        t.feed(&output(
            "c5",
            json!({"content":"failed to parse patch","success":false}),
        ));
        assert_eq!(t.fail_streak, 3);
        // The user turning a call down is not the agent failing.
        t.feed(&output("c6", json!("exec command rejected by user")));
        assert_eq!(t.fail_streak, 3);
        t.feed(&rollout(
            "event_msg",
            json!({"type":"user_message","message":"换个办法"}),
        ));
        assert_eq!(t.fail_streak, 0, "the human spoke: it starts over");
    }

    /// A finished turn's outcome is read from the agent's last words, so
    /// they must be the last text it wrote, whatever tools ran after earlier
    /// text, and gone once the human speaks: a reply to the previous turn
    /// says nothing of the new one.
    #[test]
    fn the_last_reply_is_the_agents_last_words_since_the_human_spoke() {
        let say = |text: &str| {
            json!({"type":"assistant","message":{"content":[{"type":"text","text":text}]}})
                .to_string()
        };
        let mut t = Transcript::default();
        t.feed(&user(json!("修登录页")));
        t.feed(&say("先看看代码"));
        t.feed(&call("r", "Read", json!({"file_path":"/a.ts"})));
        t.feed(&result("r", false, "code"));
        assert_eq!(
            t.last_reply.as_deref(),
            Some("先看看代码"),
            "a tool result is not the human"
        );
        t.feed(&say("已修好，\n补了 2 个测试"));
        assert_eq!(t.last_reply.as_deref(), Some("已修好， 补了 2 个测试"));
        t.feed(&user(json!("再部署一下")));
        assert_eq!(t.last_reply, None);
        // Codex: its message events, then the turn's own record of its end.
        let mut c = Transcript::default();
        c.feed(&rollout(
            "event_msg",
            json!({"type":"user_message","message":"修登录页"}),
        ));
        c.feed(&rollout(
            "event_msg",
            json!({"type":"agent_message","message":"我先看代码"}),
        ));
        assert_eq!(c.last_reply.as_deref(), Some("我先看代码"));
        c.feed(&rollout(
            "event_msg",
            json!({"type":"task_complete","turn_id":"t1","last_agent_message":"已修好"}),
        ));
        assert_eq!(c.last_reply.as_deref(), Some("已修好"));
        // A long report keeps its end, where it concludes.
        let mut l = Transcript::default();
        l.feed(&say(&format!("{}结论", "过程".repeat(KEEP_REPLY))));
        let kept = l.last_reply.unwrap();
        assert_eq!(kept.chars().count(), KEEP_REPLY);
        assert!(kept.ends_with("结论"));
    }

    #[test]
    fn the_task_is_the_humans_last_words_not_the_harness_injections() {
        let mut t = Transcript::default();
        t.feed(&user(json!("修复登录页\n的 bug")));
        t.feed(&user(json!(
            "<task-notification>agent done</task-notification>"
        )));
        t.feed(&user(json!([{"type":"tool_result","content":"ok"}])));
        t.feed(&user(json!("<system-reminder>x</system-reminder>")));
        t.feed(&json!({"type":"user","isMeta":true,"message":{"content":"Caveat"}}).to_string());
        t.feed(
            &json!({"type":"user","isCompactSummary":true,"isVisibleInTranscriptOnly":true,
            "message":{"content":"This session is being continued from a previous conversation"}})
            .to_string(),
        );
        assert_eq!(t.latest_prompt(), Some("修复登录页 的 bug"));
        t.feed(&user(
            json!([{"type":"text","text":"再加个测试"},{"type":"image"}]),
        ));
        assert_eq!(t.latest_prompt(), Some("再加个测试"));
        assert_eq!(t.prompts.len(), 2);
    }

    #[test]
    fn tools_and_the_running_todo_are_tracked() {
        let mut t = Transcript::default();
        t.feed(&tool("Edit", json!({"file_path":"/repo/src/auth.ts"})));
        t.feed(&tool(
            "TodoWrite",
            json!({"todos":[
                {"content":"a","status":"completed","activeForm":"Doing a"},
                {"content":"b","status":"in_progress","activeForm":"Writing tests"}]}),
        ));
        assert_eq!(t.todo_active.as_deref(), Some("Writing tests"));
        assert_eq!(
            t.recent_tools.front().map(String::as_str),
            Some("Edit auth.ts")
        );
        assert_eq!(t.tool_calls, 2);
        for i in 0..10 {
            t.feed(&tool("Bash", json!({"command": format!("echo {i}")})));
        }
        assert_eq!(t.recent_tools.len(), KEEP_TOOLS);
        assert_eq!(
            t.recent_tools.back().map(String::as_str),
            Some("Bash echo 9")
        );
        t.feed(&tool("Grep", json!({"pattern": "a\nb\u{1b}[2J"})));
        assert_eq!(
            t.recent_tools.back().map(String::as_str),
            Some("Grep a b [2J")
        );
    }

    fn result(id: &str, is_error: bool, content: &str) -> String {
        user(json!([{"type":"tool_result","tool_use_id":id,"is_error":is_error,"content":content}]))
    }

    fn call(id: &str, name: &str, input: Value) -> String {
        json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":name,"input":input}]}})
            .to_string()
    }

    fn result_at(id: &str, is_error: bool, ts: &str) -> String {
        json!({"type":"user","timestamp":ts,"message":{"role":"user","content":
            [{"type":"tool_result","tool_use_id":id,"is_error":is_error,"content":"x"}]}})
        .to_string()
    }

    /// The office cheers what the agent actually pulled off: a green test
    /// run, a push or a PR, and a call that goes through after it was stuck
    /// failing. A red run, a build or a plain read is no cheer, or the
    /// confetti would fly for nothing and mean nothing.
    #[test]
    fn a_green_test_run_a_push_and_a_comeback_are_cheered_and_nothing_else() {
        let ts = "2026-11-05T09:30:15.000Z";
        let at = parse_timestamp(ts).unwrap();
        let mut t = Transcript::default();
        t.feed(&call(
            "1",
            "Bash",
            json!({"command": "cargo test --release"}),
        ));
        t.feed(&result_at("1", true, ts));
        assert_eq!(t.cheer, None, "a red run");
        t.feed(&call("2", "Bash", json!({"command": "cargo build"})));
        t.feed(&result_at("2", false, ts));
        assert_eq!(t.cheer, None, "a build is not a test run");
        t.feed(&call("3", "Bash", json!({"command": "npm test"})));
        t.feed(&result_at("3", false, ts));
        assert_eq!(t.cheer, Some((Cheer::TestsPassed, at)));

        let mut t = Transcript::default();
        t.feed(&call(
            "1",
            "Bash",
            json!({"command": "git push -u origin main"}),
        ));
        t.feed(&result_at("1", false, ts));
        assert_eq!(t.cheer, Some((Cheer::Shipped, at)));
        let mut t = Transcript::default();
        t.feed(&call("1", "mcp__github__create_pull_request", json!({})));
        t.feed(&result_at("1", false, ts));
        assert_eq!(t.cheer, Some((Cheer::Shipped, at)), "a PR is shipping too");

        let mut t = Transcript::default();
        for id in ["1", "2", "3"] {
            t.feed(&call(id, "Bash", json!({"command": "cargo test"})));
            t.feed(&result_at(id, true, ts));
        }
        t.feed(&call("4", "Read", json!({"file_path": "/a.rs"})));
        t.feed(&result_at("4", false, ts));
        assert_eq!(
            t.cheer,
            Some((Cheer::Recovered, at)),
            "out of three failures in a row, even a read is a comeback"
        );
        let mut t = Transcript::default();
        for id in ["1", "2"] {
            t.feed(&call(id, "Bash", json!({"command": "ls"})));
            t.feed(&result_at(id, true, ts));
        }
        t.feed(&call("3", "Read", json!({"file_path": "/a.rs"})));
        t.feed(&result_at("3", false, ts));
        assert_eq!(t.cheer, None, "two failures are not stuck, so no comeback");
    }

    /// A Codex rollout's test run is cheered as a Claude transcript's is.
    #[test]
    fn a_codex_test_run_is_cheered_when_it_exits_zero() {
        let mut t = Transcript::default();
        t.feed(&rollout(
            "response_item",
            json!({"type":"function_call","name":"exec_command",
            "call_id":"c1","arguments":"{\"cmd\":\"pytest -q\"}"}),
        ));
        t.feed(&rollout(
            "response_item",
            json!({"type":"function_call_output","call_id":"c1",
            "output":"Process exited with code 1"}),
        ));
        assert_eq!(t.cheer, None);
        t.feed(&rollout(
            "response_item",
            json!({"type":"function_call","name":"exec_command",
            "call_id":"c2","arguments":"{\"cmd\":\"pytest -q\"}"}),
        ));
        t.feed(&rollout(
            "response_item",
            json!({"type":"function_call_output","call_id":"c2",
            "output":"Process exited with code 0"}),
        ));
        assert_eq!(t.cheer.map(|(c, _)| c), Some(Cheer::TestsPassed));
    }

    #[test]
    fn the_phase_follows_the_kind_of_tool_last_called() {
        let cases: &[(&str, Value, Phase)] = &[
            ("Grep", json!({"pattern":"x"}), Phase::Research),
            ("Edit", json!({"file_path":"/a.rs"}), Phase::Code),
            (
                "Bash",
                json!({"command":"cargo test --release"}),
                Phase::Test,
            ),
            ("Bash", json!({"command":"npx jest auth"}), Phase::Test),
            ("Bash", json!({"command":"git status"}), Phase::Run),
            ("Agent", json!({"description":"查资料"}), Phase::Delegate),
        ];
        for (name, input, want) in cases {
            let mut t = Transcript::default();
            t.feed(&tool(name, input.clone()));
            assert_eq!(t.phase, Some(*want), "{name} {input}");
        }
        // A tool that says nothing about the work keeps the phase it had.
        let mut t = Transcript::default();
        t.feed(&tool("Edit", json!({"file_path":"/a.rs"})));
        t.feed(&tool("TodoWrite", json!({"todos":[]})));
        assert_eq!(t.phase, Some(Phase::Code));
    }

    /// Errors in a row are what an agent going round in circles looks like;
    /// one success means it got past them.
    #[test]
    fn failures_count_in_a_row_and_a_success_clears_them() {
        let mut t = Transcript::default();
        for i in 0..3 {
            let id = format!("t{i}");
            t.feed(&call(&id, "Bash", json!({"command":"cargo build"})));
            assert!(t.awaits_any(&["Bash"]));
            t.feed(&result(&id, true, "error: could not compile"));
            assert!(t.outstanding.is_empty());
        }
        assert_eq!(t.fail_streak, 3);
        t.feed(&result("t3", false, "ok"));
        assert_eq!(t.fail_streak, 0);
    }

    /// Turning a call down is the user's say, not the agent failing; and a
    /// new prompt starts the count over.
    #[test]
    fn rejections_and_a_new_prompt_do_not_count_as_failing() {
        let mut t = Transcript::default();
        for i in 0..3 {
            t.feed(&result(
                &format!("r{i}"),
                true,
                "The user doesn't want to proceed with this tool use. The tool use was rejected",
            ));
        }
        assert_eq!(t.fail_streak, 0);
        t.feed(&result("e0", true, "boom"));
        t.feed(&result("e1", true, "boom"));
        t.feed(&user(json!("换个办法试试")));
        assert_eq!(t.fail_streak, 0);
    }

    /// A long call still out keeps counting as out while a parallel short one
    /// comes back: a build beside a read is still building.
    #[test]
    fn parallel_calls_are_each_tracked_by_id() {
        let mut t = Transcript::default();
        t.feed(&call("b", "Bash", json!({"command":"cargo build"})));
        t.feed(&call("r", "Read", json!({"file_path":"/a.rs"})));
        t.feed(&result("r", false, "fn main"));
        assert!(t.awaits_any(&["Bash"]));
        t.feed(&result("b", false, "Finished"));
        assert!(!t.awaits_any(&["Bash"]));
    }

    /// The card counts the turn from when the human last spoke, so it must
    /// restart on every prompt, repeated words included, and never count from
    /// a tool result or a harness injection.
    #[test]
    fn the_turn_starts_when_the_human_last_spoke() {
        let at = |ts: &str, content: Value| {
            json!({"type":"user","timestamp":ts,"message":{"content":content}}).to_string()
        };
        let secs = |t: Option<SystemTime>| {
            t.map(|t| t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs())
        };
        let mut t = Transcript::default();
        t.feed(&at("2026-11-05T09:30:15.937Z", json!("修复登录页")));
        assert_eq!(secs(t.turn_started), Some(1_793_871_015));
        t.feed(&at(
            "2026-11-05T09:40:00Z",
            json!([{"type":"tool_result","tool_use_id":"x","content":"ok"}]),
        ));
        t.feed(&at(
            "2026-11-05T09:41:00Z",
            json!("<system-reminder>x</system-reminder>"),
        ));
        assert_eq!(secs(t.turn_started), Some(1_793_871_015), "not a new turn");
        t.feed(&at("2026-11-05T17:50:00+08:00", json!("修复登录页")));
        assert_eq!(
            secs(t.turn_started),
            Some(1_793_872_200),
            "same words, new turn"
        );
        // A prompt with no time: an old start would overstate the turn.
        t.feed(&user(json!("再加个测试")));
        assert_eq!(t.turn_started, None);
    }

    #[test]
    fn timestamps_that_are_not_rfc_3339_are_refused() {
        for bad in [
            "",
            "2026-11-05",
            "2026-11-05 09:30:15Z",
            "2026-13-05T09:30:15Z",
            "2026-11-05T09:30:15",
            "2026-11-05T09:30:15.Z",
            "2026-11-05T09:30:15+0800",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(parse_timestamp(bad), None, "{bad:?}");
        }
        assert_eq!(
            parse_timestamp("2000-02-29T00:00:00.5Z"),
            Some(SystemTime::UNIX_EPOCH + Duration::new(951_782_400, 500_000_000))
        );
    }

    #[test]
    fn a_huge_transcript_is_read_from_its_end_and_still_knows_the_prompt() {
        let dir = std::env::temp_dir().join(format!("agent-office-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.jsonl");
        let filler = tool("Read", json!({"file_path": "x".repeat(1000)}));
        let mut body = format!("{}\n", user(json!("很早以前的任务")));
        while body.len() as u64 <= FIRST_READ + 4096 {
            body.push_str(&filler);
            body.push('\n');
        }
        body.push_str(&json!({"type":"last-prompt","lastPrompt":"给大文件加尾读"}).to_string());
        body.push('\n');
        std::fs::write(&path, &body).unwrap();
        let mut t = Transcript::default();
        Tail::new(path).poll(&mut t).unwrap();
        assert_eq!(t.latest_prompt(), Some("给大文件加尾读"));
        assert!(
            !t.prompts.iter().any(|p| p == "很早以前的任务"),
            "the head was skipped"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn tail_feeds_only_complete_lines_and_resumes_after_a_partial_write() {
        let dir = std::env::temp_dir().join(format!("agent-office-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let first = user(json!("第一个任务"));
        let second = user(json!("第二个任务"));
        let (head, tail) = second.split_at(10);
        std::fs::write(&path, format!("{first}\n{head}")).unwrap();
        let mut tl = Tail::new(path.clone());
        let mut t = Transcript::default();
        assert!(tl.poll(&mut t).unwrap());
        assert_eq!(t.latest_prompt(), Some("第一个任务"));
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut f, format!("{tail}\n").as_bytes()).unwrap();
        assert!(tl.poll(&mut t).unwrap());
        assert_eq!(t.latest_prompt(), Some("第二个任务"));
        assert!(!tl.poll(&mut t).unwrap());
        std::fs::remove_dir_all(dir).ok();
    }
}
