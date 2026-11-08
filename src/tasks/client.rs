//! The one-line task summary, and the one-line outcome of a finished turn,
//! from an OpenAI-compatible chat endpoint.
//! Not `claude -p`: that session would fire the hooks and walk into the office
//! as one more agent.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::text::{one_line, truncate_cells};

/// `KEY=value` lines read when the variables are not in the environment.
pub(crate) const ENV_FILE: &str = ".config/agent-office/env";
/// What a label may take: twenty CJK characters, or forty Latin ones.
pub const SUMMARY_CELLS: u16 = 40;

const SYSTEM_ZH: &str = "你给 AI 编程助手正在做的任务起一句话标题。\
根据开发者最新的指令（必要时参考之前的指令、当前 todo 和最近动作），\
用简体中文写一句动宾结构的短句，不超过20个汉字，不加句号、引号或任何前缀，只输出这一句。";

const SYSTEM_EN: &str = "You title the task an AI coding assistant is working on in one line. \
From the developer's latest prompt (and, if needed, the earlier prompts, the current todo and recent actions), \
write one short English verb phrase of at most 40 characters, with no period, quotes or prefix. Output only that phrase.";

const OUTCOME_SYSTEM_ZH: &str = "你用一句话告诉开发者 AI 编程助手这一轮的结果。\
根据它的任务和它最后的回复，用简体中文写一句短句，不超过20个汉字：\
做完了就说做成了什么；没做成就说卡在哪；回复里明确向开发者提了问题、要他回答或拿主意才能继续，就以「问：」开头说问的是什么。\
它在等自己的后台任务、子代理、CI 或部署，或只是顺带提议、留了待办，都不算问，照实说进展。\
不加句号、引号或别的前缀，只输出这一句。";

const OUTCOME_SYSTEM_EN: &str = "In one line, tell the developer how this turn of an AI coding assistant ended. \
From its task and its last reply, write one short English sentence of at most 40 characters: \
if it finished, say what it got done; if not, say where it got stuck; if the reply explicitly asks the developer a question, \
or needs their answer or decision to go on, start with \"Q: \" and say what it asks. \
Waiting on its own background tasks, subagents, CI or deploys, or merely offering a suggestion or leaving a todo, \
is not asking: report the progress as it is. \
No period, quotes or other prefix. Output only that line.";

/// The summary's instructions, in the language its line is shown in.
fn system() -> &'static str {
    crate::i18n::t(SYSTEM_ZH, SYSTEM_EN)
}

fn outcome_system() -> &'static str {
    crate::i18n::t(OUTCOME_SYSTEM_ZH, OUTCOME_SYSTEM_EN)
}

/// What a finished turn's outcome is read from.
#[derive(Debug, Clone, Default)]
pub struct OutcomeInput {
    pub project: String,
    /// The prompt the turn answered.
    pub task: Option<String>,
    /// The agent's last reply.
    pub reply: String,
}

#[derive(Debug, Clone, Default)]
pub struct SummaryInput {
    pub project: String,
    pub latest_prompt: String,
    pub earlier_prompts: Vec<String>,
    pub todo: Option<String>,
    pub recent_tools: Vec<String>,
}

/// Requests out at once, across every session: an office opening on dozens
/// of sessions, or a gateway failing for all of them, should not flood it.
const AT_ONCE: usize = 4;

#[derive(Debug, Clone)]
pub struct Summarizer {
    http: reqwest::Client,
    url: String,
    key: String,
    model: String,
    slots: Arc<tokio::sync::Semaphore>,
}

impl Summarizer {
    /// From the environment, else `~/.config/agent-office/env`. There is no
    /// default endpoint: `None` unless base, key and model are all set, or
    /// with `AGENT_OFFICE_SUMMARY=off`; nothing is then sent anywhere and the
    /// panel shows the raw prompt.
    pub fn from_env() -> Option<Self> {
        if std::env::var("AGENT_OFFICE_SUMMARY").is_ok_and(|v| v == "off") {
            return None;
        }
        let file = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(ENV_FILE))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .or_else(|| env_file_var(&file, k))
                .filter(|v| !v.is_empty())
        };
        let base = var("AGENT_OFFICE_API_BASE")?;
        let key = var("AGENT_OFFICE_API_KEY")?;
        let model = var("AGENT_OFFICE_MODEL")?;
        Some(Self::new(&base, key, model))
    }

    pub fn new(base: &str, key: String, model: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("a client with only a timeout set builds"),
            url: chat_url(base),
            key,
            model,
            slots: Arc::new(tokio::sync::Semaphore::new(AT_ONCE)),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// What [`Self::summarize`] would ask, as a [`memo`](super::memo) key.
    pub fn summary_key(&self, input: &SummaryInput) -> String {
        super::memo::key(&[&self.model, system(), &user_message(input)])
    }

    /// What [`Self::summarize_outcome`] would ask, as a [`memo`](super::memo) key.
    pub fn outcome_key(&self, input: &OutcomeInput) -> String {
        super::memo::key(&[&self.model, outcome_system(), &outcome_message(input)])
    }

    pub async fn summarize(&self, input: &SummaryInput) -> Result<String> {
        self.ask(system(), user_message(input)).await
    }

    pub async fn summarize_outcome(&self, input: &OutcomeInput) -> Result<String> {
        let line = self.ask(outcome_system(), outcome_message(input)).await?;
        Ok(unasked(line, &input.reply))
    }

    async fn ask(&self, system: &str, user: String) -> Result<String> {
        let _slot = self.slots.acquire().await.context("summary slots closed")?;
        let body = json!({
            "model": self.model,
            "max_tokens": 60,
            "temperature": 0.2,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        });
        let resp = self
            .http
            .post(&self.url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .context("summary API unreachable")?;
        let status = resp.status();
        if !status.is_success() {
            bail!("summary API HTTP {status}");
        }
        let v: Value = resp.json().await.context("summary API reply not JSON")?;
        let text = v
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        clean(text).context("summary API returned an empty summary")
    }
}

pub(crate) fn env_file_var(file: &str, key: &str) -> Option<String> {
    file.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("export ").unwrap_or(l).split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches('"').trim_matches('\'').to_owned())
}

fn user_message(input: &SummaryInput) -> String {
    use crate::i18n::t;
    let mut m = format!("{}: {}\n", t("项目", "Project"), input.project);
    for p in &input.earlier_prompts {
        m.push_str(&format!(
            "{}: {}\n",
            t("之前的指令", "Earlier prompt"),
            truncate_cells(p, 300)
        ));
    }
    m.push_str(&format!(
        "{}: {}\n",
        t("最新指令", "Latest prompt"),
        truncate_cells(&input.latest_prompt, 1500)
    ));
    if let Some(todo) = &input.todo {
        m.push_str(&format!("{}: {todo}\n", t("当前 todo", "Current todo")));
    }
    if !input.recent_tools.is_empty() {
        m.push_str(&format!(
            "{}: {}\n",
            t("最近动作", "Recent actions"),
            input.recent_tools.join("; ")
        ));
    }
    m
}

fn outcome_message(input: &OutcomeInput) -> String {
    use crate::i18n::t;
    let mut m = format!("{}: {}\n", t("项目", "Project"), input.project);
    if let Some(task) = &input.task {
        m.push_str(&format!(
            "{}: {}\n",
            t("任务", "Task"),
            truncate_cells(task, 600)
        ));
    }
    // A long reply's conclusion is at its end.
    let n = input.reply.chars().count();
    let tail: String = input.reply.chars().skip(n.saturating_sub(1500)).collect();
    m.push_str(&format!("{}: {tail}\n", t("最后的回复", "Last reply")));
    m
}

/// `line` without its 「问：」 when `reply` asks nothing: a 「问：」 lines the
/// session up as waiting on you and banners it, and the model also reads one
/// into a turn that ended waiting on its own background work. A reply asks
/// with a question mark, or by handing you the call in words at its end
/// (「还要你定两件事：…」 ends on a full stop and still waits on you).
fn unasked(line: String, reply: &str) -> String {
    let tail: String = {
        let n = reply.chars().count();
        reply.chars().skip(n.saturating_sub(ASK_TAIL)).collect()
    };
    // A full-width mark reports a question — but one in quotes is prose, the
    // question a reply says it already traced or answered. A plain `?`
    // mid-reply is as often code (`foo()?`) or a URL, so only the ending's
    // counts.
    let lower = tail.to_lowercase();
    let asks = unquoted(reply).contains('？')
        || tail.contains('?')
        || handed_over(&tail, &ASKS_YOU)
        || handed_over(&lower, &ASKS_YOU_EN);
    match crate::i18n::strip_ask(&line) {
        Some(rest) if !asks => rest.trim().to_owned(),
        _ => line,
    }
}

/// `text` with every quoted span dropped: what a reply quotes is something
/// already said, not the mark of it asking you.
fn unquoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quoted = 0usize;
    for c in text.chars() {
        match c {
            '「' | '『' | '“' | '‘' => quoted += 1,
            '」' | '』' | '”' | '’' => quoted = quoted.saturating_sub(1),
            _ if quoted == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Whether `text` hands you a decision anywhere: one of `words` in a place
/// that is not a RECOUNT of the one you made.
fn handed_over(text: &str, words: &[&str]) -> bool {
    words
        .iter()
        .any(|w| text.match_indices(w).any(|(at, _)| !recount(&text[..at])))
}

/// Whether the words before a hand-off turn it into a recount of a decision
/// already made — 「按你决定的方案」, 「方案已拍板」, "as you decided" — which
/// closes a turn that waits on nothing.
fn recount(before: &str) -> bool {
    const RECOUNT: [&str; 7] = ["按", "照", "依", "听", "已", "as", "per"];
    let before = before.trim_end();
    RECOUNT.iter().any(|m| before.ends_with(m))
}

/// How far from a reply's end [`ASKS_YOU`] is looked for: a hand-off closes
/// the reply, and the same words earlier on recount what was decided.
const ASK_TAIL: usize = 200;

/// Words that hand the developer a decision. Not 「告诉我」 or 「需要你」:
/// replies close with 「有问题告诉我」 and 「不需要你做什么」 asking nothing.
const ASKS_YOU: [&str; 8] = [
    "要你定",
    "你来定",
    "你定一下",
    "你决定",
    "你选",
    "等你点头",
    "拍板",
    "请确认",
];

/// [`ASKS_YOU`] in English, lower-cased; the reply's language is the
/// session's, not the office's. Not "let me know": replies close with "let
/// me know if anything breaks" asking nothing.
const ASKS_YOU_EN: [&str; 5] = [
    "your call",
    "you decide",
    "up to you",
    "please confirm",
    "need your decision",
];

/// The model's line as a label: one line, no quotes or closing stop, capped.
fn clean(raw: &str) -> Option<String> {
    let line = one_line(raw.lines().find(|l| !l.trim().is_empty())?);
    let stops = ['。', '.', '！', '!'];
    let line = line
        .trim_end_matches(stops)
        .trim_matches(|c: char| "\"'“”‘’「」《》".contains(c))
        .trim_end_matches(stops)
        .trim();
    let line = line
        .strip_prefix("标题：")
        .or_else(|| line.strip_prefix("任务："))
        .or_else(|| line.strip_prefix("Title:"))
        .or_else(|| line.strip_prefix("Task:"))
        .unwrap_or(line)
        .trim();
    (!line.is_empty()).then(|| truncate_cells(line, SUMMARY_CELLS))
}

/// `<base>/v1/chat/completions`, unless the base's path already names an API
/// version (`/v1`, `/api/v3`, `/api/paas/v4`, `/v1beta/openai`): such a
/// service serves chat under that version, and another `/v1` would 404.
fn chat_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = base.split_once("://").map_or(base, |(_, rest)| rest);
    let versioned = path.split('/').skip(1).any(|seg| {
        seg.strip_prefix('v').is_some_and(|rest| {
            let tag = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            tag.len() < rest.len() && tag.chars().all(|c| c.is_ascii_alphanumeric())
        })
    });
    if versioned {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare base gets `/v1` as before; a base that already names its
    /// version is used as given, or services on `/api/v3` and the like 404.
    #[test]
    fn the_chat_url_adds_v1_only_when_the_base_names_no_version() {
        for (base, url) in [
            ("https://x", "https://x/v1/chat/completions"),
            ("https://x/", "https://x/v1/chat/completions"),
            ("https://x/v1", "https://x/v1/chat/completions"),
            ("https://x/v1/", "https://x/v1/chat/completions"),
            ("https://x/openai", "https://x/openai/v1/chat/completions"),
            ("https://x/api/v3", "https://x/api/v3/chat/completions"),
            ("https://x/api/paas/v4", "https://x/api/paas/v4/chat/completions"),
            ("https://x/v1beta/openai", "https://x/v1beta/openai/chat/completions"),
            // A host that looks like a version is not a path segment.
            ("https://v2.x", "https://v2.x/v1/chat/completions"),
        ] {
            assert_eq!(chat_url(base), url, "{base}");
        }
    }

    #[test]
    fn model_chatter_is_trimmed_to_a_label() {
        assert_eq!(
            clean("「修复登录页 bug」。\n").as_deref(),
            Some("修复登录页 bug")
        );
        assert_eq!(clean("任务：调研两个仓库").as_deref(), Some("调研两个仓库"));
        assert_eq!(clean("  \n"), None);
        let long = "一".repeat(40);
        assert!(crate::text::cells(&clean(&long).unwrap()) <= SUMMARY_CELLS);
    }

    /// A long report concludes at its end: that part must reach the model,
    /// with the task it answers.
    #[test]
    fn an_outcome_request_carries_the_task_and_the_replys_end() {
        let reply = format!("{}结论：登录页已修好", "过程。".repeat(2000));
        let m = outcome_message(&OutcomeInput {
            project: "web".into(),
            task: Some("修登录页".into()),
            reply,
        });
        assert!(m.contains("任务: 修登录页"));
        assert!(m.ends_with("结论：登录页已修好\n"));
        assert!(m.chars().count() < 1700);
    }

    /// A turn that ended waiting on its own subagent asks you nothing: it must
    /// not be bannered as waiting on your reply. One that does ask keeps it.
    #[test]
    fn a_question_needs_one_in_the_reply() {
        assert_eq!(
            unasked(
                "问：能否提供最新界面截图".into(),
                "我在等代码梳理的结果。截图还是首个版本时拍的。"
            ),
            "能否提供最新界面截图"
        );
        assert_eq!(
            unasked("问：要合并吗".into(), "测试都过了，要合并到 main 吗？"),
            "问：要合并吗"
        );
        assert_eq!(unasked("登录页已修好".into(), "修好了"), "登录页已修好");
    }

    /// A reply that hands you a decision without a question mark still waits
    /// on you: dropping its 「问：」 filed a turn waiting on a spending call as
    /// done, with no banner and no place in the queue.
    #[test]
    fn a_decision_handed_over_in_words_is_still_a_question() {
        let reply = "先走 A + 不签名。\n\n还要你定两件事：仓库是公开（A）还是另建公开仓库（B），以及要不要花 99 美元/年做签名。";
        assert_eq!(
            unasked("问：仓库公开还是另建，是否花99美元签名".into(), reply),
            "问：仓库公开还是另建，是否花99美元签名"
        );
        // The same words long before the end recount a call already made.
        let recount = format!("按你决定的方案做完了。{}已推送。", "测试全过，".repeat(60));
        assert_eq!(unasked("问：要发版吗".into(), &recount), "要发版吗");
        // A sign-off and a `?` in code ask nothing of a turn waiting on CI.
        let waits = format!(
            "改成 `let x = foo()?;`。{}还在等 CI，有问题告诉我。",
            "已推送，".repeat(60)
        );
        assert_eq!(unasked("问：CI 结果".into(), &waits), "CI 结果");
    }

    /// A finished report must not line the session up as waiting on your
    /// reply. The model prefixes 「问：」onto a turn that only recounts a
    /// decision already made, or quotes a question it already answered; a
    /// short reply holds that recount in its ending, and a full-width mark
    /// inside an earlier sentence is not the developer being asked. A real
    /// question, or a decision still handed over, keeps the mark: dropping
    /// every 「问：」would hide the turns that do wait.
    #[test]
    fn a_finished_report_is_not_a_question_to_you() {
        for (line, reply) in [
            ("问：要不要改", "按你决定的方案做完了，已推送。"),
            ("问：发不发", "方案已拍板，代码已推送。"),
            ("问：选哪个", "按你选的 A 方案改完了。"),
            (
                "问：为什么挂了",
                "我顺着日志里的「为什么挂了？」查完了。断言写反，已经改完并推送。",
            ),
            ("Q: release it", "As you decided, it's pushed."),
        ] {
            assert_eq!(
                unasked(line.into(), reply),
                crate::i18n::strip_ask(line).unwrap(),
                "{reply}"
            );
        }
        assert_eq!(
            unasked("问：可以合并吗".into(), "测试过了。可以合并吗？"),
            "问：可以合并吗"
        );
        assert_eq!(
            unasked("问：发不发版".into(), "改完了，请确认是否发版。"),
            "问：发不发版"
        );
        assert_eq!(
            unasked("问：发不发".into(), "两套方案都写在上面，拍板吧。"),
            "问：发不发"
        );
    }

    /// An English office must get English lines, and not the Chinese a
    /// memo kept for the same session: the language is part of the request.
    #[test]
    fn the_request_follows_the_offices_language() {
        use crate::i18n::{Lang, with};
        let s = Summarizer::new("https://x", "k".into(), "m".into());
        let input = SummaryInput {
            project: "web".into(),
            latest_prompt: "fix login".into(),
            ..Default::default()
        };
        let en = with(Lang::En, || {
            assert!(system().contains("English"));
            assert!(user_message(&input).starts_with("Project: web\n"));
            s.summary_key(&input)
        });
        assert_ne!(en, with(Lang::Zh, || s.summary_key(&input)));
    }

    /// An English question lines the session up as waiting on you just as a
    /// Chinese one does, and loses its mark the same way when nothing is asked.
    #[test]
    fn an_english_question_needs_one_in_the_reply() {
        assert_eq!(
            unasked("Q: merge it".into(), "Tests pass. Merge to main?"),
            "Q: merge it"
        );
        assert_eq!(
            unasked("Q: pick a host".into(), "Two options above. Your call."),
            "Q: pick a host"
        );
        assert_eq!(
            unasked("Q: CI result".into(), "Pushed; waiting on CI."),
            "CI result"
        );
    }

    #[test]
    fn env_file_values_are_read_without_quotes_or_comments() {
        let f = "# AGENT_OFFICE_API_KEY=old\nexport AGENT_OFFICE_API_KEY=\"sk-new\"\nAGENT_OFFICE_API_BASE=https://x/v1\n";
        assert_eq!(
            env_file_var(f, "AGENT_OFFICE_API_KEY").as_deref(),
            Some("sk-new")
        );
        let s = Summarizer::new("https://x/v1/", "k".into(), "m".into());
        assert_eq!(s.url, "https://x/v1/chat/completions");
    }

    #[tokio::test]
    async fn an_unreachable_gateway_is_an_error_not_a_hang() {
        let s = Summarizer::new("http://127.0.0.1:9", "k".into(), "m".into());
        let input = SummaryInput {
            latest_prompt: "x".into(),
            ..Default::default()
        };
        assert!(s.summarize(&input).await.is_err());
    }
}
