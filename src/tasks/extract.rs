//! What a Claude Code transcript says the agent was asked to do. pixtuoid's
//! decoder keeps only tool calls, so the prompts and todos come from here.
//! agent-office's dsh plugin writes its task logs in the same shape, with
//! dsh's own tool names (`bash`, `read`, `todo_write`, ...).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::text::{one_line, truncate_cells};

const KEEP_PROMPTS: usize = 3;
/// How much of a transcript the first read takes from its end: sessions run
/// to 100 MB, nearly all of it tool output.
const FIRST_READ: u64 = 4 << 20;
const KEEP_TOOLS: usize = 5;

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
        "edit" | "write" => Phase::Code,
        // One tool for looking and changing; `view` only reads.
        "str_replace_editor" if input.get("command").and_then(Value::as_str) == Some("view") => {
            Phase::Research
        }
        "str_replace_editor" => Phase::Code,
        "Task" | "Agent" | "subagent" | "subagent_fork" => Phase::Delegate,
        "Bash" | "bash" => {
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
        // The human spoke: whatever went wrong before, the agent starts over.
        if human_prompt(&text).is_some() {
            self.fail_streak = 0;
        }
        self.push_prompt(&text);
    }

    fn feed_assistant(&mut self, v: &Value) {
        let Some(Value::Array(blocks)) = v.pointer("/message/content") else {
            return;
        };
        for b in blocks {
            if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
            let input = b.get("input").unwrap_or(&Value::Null);
            self.tool_calls += 1;
            if let Some(id) = b.get("id").and_then(Value::as_str) {
                if self.outstanding.len() == KEEP_OUTSTANDING {
                    self.outstanding.remove(0);
                }
                self.outstanding.push((id.to_owned(), name.to_owned()));
            }
            if let Some(p) = phase_of(name, input) {
                self.phase = Some(p);
            }
            if name == "TodoWrite" || name == "todo_write" {
                self.todo_active = active_todo(input);
            }
            if self.recent_tools.len() == KEEP_TOOLS {
                self.recent_tools.pop_front();
            }
            self.recent_tools.push_back(tool_line(name, input));
        }
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
        | "edit" | "write" | "str_replace_editor" => s("file_path")
            .or_else(|| s("notebook_path"))
            .or_else(|| s("path"))
            .map(|p| {
                Path::new(p)
                    .file_name()
                    .map_or_else(|| p.to_owned(), |f| f.to_string_lossy().into_owned())
            }),
        "Bash" | "bash" => s("description").or_else(|| s("command")).map(str::to_owned),
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
