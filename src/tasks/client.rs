//! The one-line task summary from an OpenAI-compatible chat endpoint.
//! Not `claude -p`: that session would fire the hooks and walk into the office
//! as one more agent.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::text::{one_line, truncate_cells};

/// `KEY=value` lines read when the variables are not in the environment.
const ENV_FILE: &str = ".config/agent-office/env";
/// What a label may take: twenty CJK characters.
pub const SUMMARY_CELLS: u16 = 40;

const SYSTEM: &str = "你给 AI 编程助手正在做的任务起一句话标题。\
根据开发者最新的指令（必要时参考之前的指令、当前 todo 和最近动作），\
用简体中文写一句动宾结构的短句，不超过20个汉字，不加句号、引号或任何前缀，只输出这一句。";

#[derive(Debug, Clone, Default)]
pub struct SummaryInput {
    pub project: String,
    pub latest_prompt: String,
    pub earlier_prompts: Vec<String>,
    pub todo: Option<String>,
    pub recent_tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Summarizer {
    http: reqwest::Client,
    url: String,
    key: String,
    model: String,
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
        let base = base.trim_end_matches('/').trim_end_matches("/v1");
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("a client with only a timeout set builds"),
            url: format!("{base}/v1/chat/completions"),
            key,
            model,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn summarize(&self, input: &SummaryInput) -> Result<String> {
        let body = json!({
            "model": self.model,
            "max_tokens": 60,
            "temperature": 0.2,
            "messages": [
                {"role": "system", "content": SYSTEM},
                {"role": "user", "content": user_message(input)},
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

fn env_file_var(file: &str, key: &str) -> Option<String> {
    file.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("export ").unwrap_or(l).split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches('"').trim_matches('\'').to_owned())
}

fn user_message(input: &SummaryInput) -> String {
    let mut m = format!("项目: {}\n", input.project);
    for p in &input.earlier_prompts {
        m.push_str(&format!("之前的指令: {}\n", truncate_cells(p, 300)));
    }
    m.push_str(&format!(
        "最新指令: {}\n",
        truncate_cells(&input.latest_prompt, 1500)
    ));
    if let Some(t) = &input.todo {
        m.push_str(&format!("当前 todo: {t}\n"));
    }
    if !input.recent_tools.is_empty() {
        m.push_str(&format!("最近动作: {}\n", input.recent_tools.join("; ")));
    }
    m
}

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
        .unwrap_or(line)
        .trim();
    (!line.is_empty()).then(|| truncate_cells(line, SUMMARY_CELLS))
}

#[cfg(test)]
mod tests {
    use super::*;

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
