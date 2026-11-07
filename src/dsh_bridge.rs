//! The local file bridge to agent-office's DeepSeek Harness plugin. Requests
//! and replies appear by rename, so neither side ever parses a partial JSON
//! document. The plugin owns Session creation; this side only writes and waits.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const ROLES: &[&str] = &[
    "product",
    "ux",
    "architect",
    "frontend",
    "backend",
    "qa",
    "devops",
    "reviewer",
    "pm",
];
const POLL: Duration = Duration::from_millis(25);

/// One role dispatch for the DSH plugin. `id` is also the request file stem,
/// so callers must give each attempt a fresh id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispatchRequest {
    pub id: String,
    pub role: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub submit: bool,
}

/// The Session DSH created for a successful dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchResult {
    pub session_id: String,
    pub agent_preset: String,
}

#[derive(Debug, Deserialize)]
struct WireResult {
    id: String,
    ok: bool,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "agentPreset")]
    agent_preset: Option<String>,
    error: Option<String>,
}

fn start_hint() -> String {
    crate::tr!(
        "请确认 DeepSeek Harness 正在运行；若插件未更新，请重新运行 agent-office connect dsh，然后重启 DSH",
        "make sure DeepSeek Harness is running; if the plugin is stale, run agent-office connect dsh again and restart DSH"
    )
}

fn validate(request: &DispatchRequest) -> Result<()> {
    let id = request.id.as_bytes();
    let valid_id = !id.is_empty()
        && id.len() <= 128
        && id[0].is_ascii_alphanumeric()
        && id
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !request.id.ends_with(".result");
    anyhow::ensure!(
        valid_id,
        crate::tr!("派活 id 不合法", "invalid dispatch id")
    );
    anyhow::ensure!(
        ROLES.contains(&request.role.as_str()),
        crate::tr!(
            "未知派活角色：{}",
            "unknown dispatch role: {}",
            request.role
        )
    );
    anyhow::ensure!(
        !request.cwd.as_os_str().is_empty(),
        crate::tr!("派活目录不能为空", "dispatch cwd cannot be empty")
    );
    Ok(())
}

fn request_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn result_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.result.json"))
}

fn claim_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.claim.json"))
}

/// None is an unclaimed request; Some(false) means its Host process died.
/// Unreadable or older claims fail safe: never turn an uncertain Host into an
/// invitation to dispatch the same task again.
fn claim_alive(path: &Path) -> Option<bool> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some(true),
    };
    let pid = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("pid").and_then(serde_json::Value::as_i64))
        .and_then(|pid| i32::try_from(pid).ok());
    let Some(pid) = pid else { return Some(true) };
    #[cfg(unix)]
    {
        let Some(pid) = rustix::process::Pid::from_raw(pid) else {
            return Some(true);
        };
        match rustix::process::test_kill_process(pid) {
            Ok(()) => Some(true),
            Err(e) if e == rustix::io::Errno::SRCH => Some(false),
            Err(_) => Some(true),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Some(true)
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| crate::tr!("建 {} 失败", "could not create {}", dir.display()))?;
    #[cfg(unix)]
    std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .with_context(|| crate::tr!("设置 {} 权限失败", "could not chmod {}", dir.display()))?;
    Ok(())
}

fn read_result(path: &Path, id: &str) -> Result<WireResult> {
    let raw = std::fs::read(path)
        .with_context(|| crate::tr!("读 {} 失败", "could not read {}", path.display()))?;
    let result: WireResult = serde_json::from_slice(&raw).with_context(|| {
        crate::tr!(
            "DSH 派活结果不是合法 JSON；{}",
            "invalid DSH dispatch result JSON; {}",
            start_hint()
        )
    })?;
    anyhow::ensure!(
        result.id == id,
        crate::tr!(
            "DSH 派活结果 id 不匹配；{}",
            "DSH dispatch result id does not match; {}",
            start_hint()
        )
    );
    Ok(result)
}

fn ready_result(path: &Path, id: &str) -> Result<Option<WireResult>> {
    match read_result(path, id) {
        Ok(result) => Ok(Some(result)),
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn retract_request(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e)
            .with_context(|| crate::tr!("撤回 {} 失败", "could not retract {}", path.display())),
    }
}

/// Identifies the one dispatch failure whose request and result must remain
/// available for reconciliation and a Host that is still completing its work.
#[derive(Debug)]
struct RetractionFailed;

impl std::fmt::Display for RetractionFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::tr!(
            "DSH 撤回派活请求失败：状态未知，先检查再重试",
            "could not retract the DSH request; outcome unknown: check before retrying"
        ))
    }
}

impl std::error::Error for RetractionFailed {}

fn wait_result(
    path: &Path,
    id: &str,
    request: &Path,
    claim: &Path,
    timeout: Duration,
) -> Result<WireResult> {
    wait_result_with(
        claim,
        timeout,
        || retract_request(request),
        || ready_result(path, id),
    )
}

fn wait_result_with(
    claim: &Path,
    timeout: Duration,
    mut retract: impl FnMut() -> Result<()>,
    mut ready: impl FnMut() -> Result<Option<WireResult>>,
) -> Result<WireResult> {
    let started = Instant::now();
    let mut seen_claim = false;
    loop {
        if let Some(result) = ready()? {
            return Ok(result);
        }
        // Timeout can retract only work no Host accepted. Once claimed, wait
        // for its session id instead of telling the user it is safe to retry.
        let mut alive = claim_alive(claim);
        seen_claim |= alive.is_some();
        if started.elapsed() >= timeout && alive.is_none() {
            // Retraction is the ordering point: a Host claims before reading
            // the request, so only a request gone before any claim is provably
            // unaccepted. Never leave this window to the caller's cleanup.
            retract().map_err(|error| error.context(RetractionFailed))?;
            alive = claim_alive(claim);
            seen_claim |= alive.is_some();
            // A Host writes its result BEFORE releasing its claim. If a claim
            // from before retraction has just disappeared, its result is thus
            // already durable; reading the result after the claim cannot miss
            // that transition between the two checks.
            if let Some(result) = ready()? {
                return Ok(result);
            }
            if alive.is_none() {
                if seen_claim {
                    bail!(crate::tr!(
                        "DSH 已接受派活，但结果丢失且状态未知：请先检查 DSH 会话列表，勿盲目重试",
                        "the DSH Host accepted the dispatch, but its result is unavailable: check its session list before retrying"
                    ));
                }
                bail!(crate::tr!(
                    "DSH 尚未接受派活，等待超时（{} 毫秒）；{}",
                    "the DSH Host did not accept the dispatch before the {}ms timeout; {}",
                    timeout.as_millis(),
                    start_hint()
                ));
            }
        }
        if alive == Some(false) {
            bail!(crate::tr!(
                "DSH Host 在派活中退出，结果未知：可能已创建会话，请先检查 DSH 会话列表，勿盲目重试",
                "the DSH Host exited during dispatch; the outcome is unknown: check its session list before retrying in case the session was created"
            ));
        }
        std::thread::sleep(POLL);
    }
}

fn dispatch_in(dir: &Path, request: DispatchRequest, timeout: Duration) -> Result<DispatchResult> {
    dispatch_in_with(dir, request, timeout, wait_result)
}

fn dispatch_in_with(
    dir: &Path,
    request: DispatchRequest,
    timeout: Duration,
    wait: impl FnOnce(&Path, &str, &Path, &Path, Duration) -> Result<WireResult>,
) -> Result<DispatchResult> {
    validate(&request)?;
    create_private_dir(dir)?;
    let request_path = request_path(dir, &request.id);
    let result_path = result_path(dir, &request.id);
    // A caller that reuses an id must not consume the previous attempt's
    // reply before its new request reaches the plugin.
    match std::fs::remove_file(&result_path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| {
                crate::tr!("删 {} 失败", "could not remove {}", result_path.display())
            });
        }
        _ => {}
    }
    let tmp = dir.join(format!("{}.json.tmp", request.id));
    // A queued request is not permission to work forever. If this process
    // dies before cleanup, a Host started later still refuses the old task.
    let expires_at = (std::time::SystemTime::now() + timeout)
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let expires_at = u64::try_from(expires_at).unwrap_or(u64::MAX);
    let mut json = serde_json::to_value(&request)?;
    json["expiresAt"] = expires_at.into();
    let json = serde_json::to_vec(&json)?;
    crate::atomic::write_private(&request_path, &tmp, &json).with_context(|| {
        crate::tr!(
            "写 DSH 派活请求到 {} 失败",
            "could not write the DSH dispatch request to {}",
            request_path.display()
        )
    })?;

    let attempted = (|| {
        let wire = wait(
            &result_path,
            &request.id,
            &request_path,
            &claim_path(dir, &request.id),
            timeout,
        )?;
        if !wire.ok {
            let reason = wire.error.as_deref().unwrap_or("unknown error");
            if let Some(session_id) = wire.session_id.as_deref().filter(|id| !id.is_empty()) {
                bail!(crate::tr!(
                    "DSH 已创建会话 {}（{}），但任务未提交：{}。打开这个会话继续，不要重复派活",
                    "DSH created session {} ({}), but did not submit the task: {}. Continue in that session; do not redispatch",
                    session_id,
                    wire.agent_preset.as_deref().unwrap_or("office role"),
                    reason,
                ));
            }
            bail!(crate::tr!(
                "DSH 派活失败：{}",
                "DSH dispatch failed: {}",
                reason
            ));
        }
        let session_id = wire.session_id.filter(|s| !s.is_empty());
        let agent_preset = wire.agent_preset.filter(|s| !s.is_empty());
        match (session_id, agent_preset) {
            (Some(session_id), Some(agent_preset)) => Ok(DispatchResult {
                session_id,
                agent_preset,
            }),
            _ => bail!(crate::tr!(
                "DSH 派活结果缺少 sessionId 或 agentPreset；{}",
                "DSH dispatch result is missing sessionId or agentPreset; {}",
                start_hint()
            )),
        }
    })();
    // Successful retraction proves a later Host cannot replay the request. A
    // failed retraction proves nothing: preserve both files so a Host can still
    // finish and the user can recover any session id before choosing a retry.
    let preserve_evidence = attempted
        .as_ref()
        .is_err_and(|error| error.downcast_ref::<RetractionFailed>().is_some());
    if !preserve_evidence {
        let _ = std::fs::remove_file(&request_path);
        let _ = std::fs::remove_file(&result_path);
    }
    attempted
}

/// Ask the running DSH Host to create a Session under `office-<role>`, waiting
/// at most `timeout` for its reply. The caller normally supplies ten seconds.
pub fn dispatch(request: DispatchRequest, timeout: Duration) -> Result<DispatchResult> {
    let dir = crate::dsh::home()
        .map(|home| home.join("agent-office/dispatch"))
        .context(crate::tr!(
            "找不到 dsh 目录（DSH_HOME 或 ~/.dsh）",
            "cannot find the dsh directory (DSH_HOME or ~/.dsh)"
        ))?;
    dispatch_in(&dir, request, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-office-dsh-bridge-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(id: &str) -> DispatchRequest {
        DispatchRequest {
            id: id.into(),
            role: "qa".into(),
            cwd: PathBuf::from("/repo"),
            prompt: "[office:qa]\n任务：跑测试".into(),
            submit: true,
        }
    }

    fn write_result(dir: &Path, id: &str, value: serde_json::Value) {
        let path = result_path(dir, id);
        let tmp = dir.join(format!("{id}.result.json.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(&value).unwrap()).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
    }

    fn wait_request(dir: &Path, id: &str) -> PathBuf {
        let path = request_path(dir, id);
        for _ in 0..200 {
            if path.exists() {
                return path;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("request {id} was never published");
    }

    /// Only a missing claim proves that no Host accepted the request. Read
    /// errors, malformed contents and live pids must all fail safe; a directory
    /// at the claim path exercises a real non-NotFound filesystem error.
    #[test]
    fn only_a_missing_or_dead_claim_is_not_alive() {
        let dir = scratch("claim-read-errors");
        let claim = claim_path(&dir, "req-claim-read-errors");
        assert_eq!(claim_alive(&claim), None);
        std::fs::create_dir(&claim).unwrap();
        assert_eq!(claim_alive(&claim), Some(true));
        std::fs::remove_dir(&claim).unwrap();
        std::fs::write(&claim, "not JSON\n").unwrap();
        assert_eq!(claim_alive(&claim), Some(true));
        std::fs::write(
            &claim,
            serde_json::json!({"pid":std::process::id()}).to_string(),
        )
        .unwrap();
        assert_eq!(claim_alive(&claim), Some(true));
        #[cfg(unix)]
        {
            std::fs::write(&claim, r#"{"pid":2147483647}"#).unwrap();
            assert_eq!(claim_alive(&claim), Some(false));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Permission failure is also uncertainty about a live Host, not proof of
    /// absence. Root can still read a mode-000 file, so that environment uses
    /// the directory case above while this checks the ordinary unprivileged path.
    #[cfg(unix)]
    #[test]
    fn a_permission_denied_claim_fails_safe() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("claim-permission-error");
        let claim = claim_path(&dir, "req-claim-permission-error");
        std::fs::write(
            &claim,
            serde_json::json!({"pid":std::process::id()}).to_string(),
        )
        .unwrap();
        std::fs::set_permissions(&claim, std::fs::Permissions::from_mode(0o000)).unwrap();
        let denied = std::fs::read_to_string(&claim).is_err();
        let alive = claim_alive(&claim);
        std::fs::set_permissions(&claim, std::fs::Permissions::from_mode(0o600)).unwrap();
        if denied {
            assert_eq!(alive, Some(true));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_request_and_its_success_result_follow_the_file_protocol() {
        let dir = scratch("success");
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            let path = wait_request(&worker_dir, "req-1");
            let raw = std::fs::read(&path).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            let expires_at = value["expiresAt"].as_u64().expect("an absolute expiry");
            assert!(expires_at > 0);
            assert_eq!(
                value,
                serde_json::json!({
                    "id": "req-1",
                    "role": "qa",
                    "cwd": "/repo",
                    "prompt": "[office:qa]\n任务：跑测试",
                    "submit": true,
                    "expiresAt": expires_at,
                })
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&worker_dir).unwrap().permissions().mode() & 0o777,
                    0o700
                );
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            write_result(
                &worker_dir,
                "req-1",
                serde_json::json!({
                    "id": "req-1",
                    "ok": true,
                    "sessionId": "session-1",
                    "agentPreset": "office-qa",
                }),
            );
        });
        let result = dispatch_in(&dir, request("req-1"), Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert_eq!(
            result,
            DispatchResult {
                session_id: "session-1".into(),
                agent_preset: "office-qa".into(),
            }
        );
        assert!(!request_path(&dir, "req-1").exists());
        assert!(!result_path(&dir, "req-1").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_plugin_error_comes_back_without_losing_its_reason() {
        let dir = scratch("plugin-error");
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            wait_request(&worker_dir, "req-2");
            write_result(
                &worker_dir,
                "req-2",
                serde_json::json!({"id":"req-2","ok":false,"error":"preset is broken"}),
            );
        });
        let error = dispatch_in(&dir, request("req-2"), Duration::from_secs(1)).unwrap_err();
        worker.join().unwrap();
        assert!(error.to_string().contains("preset is broken"), "{error:#}");
        assert!(!request_path(&dir, "req-2").exists());
        assert!(!result_path(&dir, "req-2").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// create succeeding and prompt failing is partial success: hiding the
    /// session id would send the user into a duplicate retry with no recovery.
    #[test]
    fn a_created_but_unsubmitted_session_is_reported_and_the_request_retracted() {
        let dir = scratch("partial");
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            while !request_path(&worker_dir, "req-partial").exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            write_result(
                &worker_dir,
                "req-partial",
                serde_json::json!({"id":"req-partial","ok":false,"error":"prompt rejected",
                    "sessionId":"session-created","agentPreset":"office-qa"}),
            );
        });
        let error = dispatch_in(&dir, request("req-partial"), Duration::from_secs(1)).unwrap_err();
        worker.join().unwrap();
        let text = format!("{error:#}");
        assert!(text.contains("session-created"), "{text}");
        assert!(text.contains("任务未提交"), "{text}");
        assert!(text.contains("prompt rejected"), "{text}");
        assert!(!request_path(&dir, "req-partial").exists());
        assert!(!result_path(&dir, "req-partial").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_timeout_says_how_to_make_the_bridge_available() {
        let dir = scratch("timeout");
        let error = dispatch_in(&dir, request("req-3"), Duration::from_millis(20)).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("超时"), "{text}");
        assert!(text.contains("agent-office connect dsh"), "{text}");
        assert!(text.contains("重启 DSH"), "{text}");
        assert!(
            !request_path(&dir, "req-3").exists(),
            "a timeout must retract the request"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A claimed prompt may finish after the initial wait budget. Returning at
    /// that budget without its session id would invite a duplicate real task.
    #[test]
    fn a_claimed_request_keeps_waiting_for_its_late_session_id() {
        let dir = scratch("late-after-timeout");
        std::fs::write(
            claim_path(&dir, "req-late"),
            serde_json::json!({"pid":std::process::id()}).to_string(),
        )
        .unwrap();
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            write_result(
                &worker_dir,
                "req-late",
                serde_json::json!({"id":"req-late","ok":true,
                    "sessionId":"session-late","agentPreset":"office-qa"}),
            );
        });
        let started = Instant::now();
        let result = dispatch_in(&dir, request("req-late"), Duration::from_millis(20)).unwrap();
        worker.join().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(60));
        assert_eq!(result.session_id, "session-late");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The Host renames the result, then releases its claim. A result arriving
    /// between our first read and the claim read must be read again, never
    /// mistaken for a request that was never accepted.
    #[test]
    fn a_result_landing_while_the_claim_disappears_is_read_again() {
        let mut reads = 0;
        let result = wait_result_with(
            Path::new("/missing-claim"),
            Duration::ZERO,
            || Ok(()),
            || {
                reads += 1;
                if reads == 1 {
                    return Ok(None);
                }
                Ok(Some(WireResult {
                    id: "req-race".into(),
                    ok: true,
                    session_id: Some("session-finished".into()),
                    agent_preset: Some("office-qa".into()),
                    error: None,
                }))
            },
        )
        .unwrap();
        assert_eq!(reads, 2);
        assert_eq!(result.session_id.as_deref(), Some("session-finished"));
    }

    /// A claim can appear after the last poll but before request retraction.
    /// The request was accepted, so the timeout must keep waiting for its real
    /// session id instead of reporting that a retry is safe.
    #[test]
    fn a_claim_created_before_retraction_keeps_waiting_for_success() {
        let dir = scratch("claim-before-retract");
        let id = "req-claim-before-retract";
        let request = request_path(&dir, id);
        let claim = claim_path(&dir, id);
        std::fs::write(&request, "{}\n").unwrap();
        let retracted = std::cell::Cell::new(false);
        let host_read_request = std::cell::Cell::new(false);
        let mut reads = 0;
        let result = wait_result_with(
            &claim,
            Duration::ZERO,
            || {
                // Model a Host that wins the old race: claim and read first.
                std::fs::write(
                    &claim,
                    serde_json::json!({"pid":std::process::id()}).to_string(),
                )
                .unwrap();
                host_read_request.set(std::fs::read(&request).is_ok());
                retract_request(&request)?;
                retracted.set(true);
                Ok(())
            },
            || {
                reads += 1;
                if reads <= 2 {
                    return Ok(None);
                }
                assert!(retracted.get());
                Ok(Some(WireResult {
                    id: id.into(),
                    ok: true,
                    session_id: Some("session-accepted".into()),
                    agent_preset: Some("office-qa".into()),
                    error: None,
                }))
            },
        )
        .unwrap();
        assert!(host_read_request.get());
        assert!(retracted.get());
        assert_eq!(reads, 3);
        assert_eq!(result.session_id.as_deref(), Some("session-accepted"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// readdirSync can return a request just before the client retracts it.
    /// A claim published afterwards can only observe the missing request; its
    /// cancellation result must not be mistaken for an executed Session.
    #[test]
    fn a_claim_created_after_retraction_cannot_execute_the_request() {
        let dir = scratch("claim-after-retract");
        let id = "req-claim-after-retract";
        let request = request_path(&dir, id);
        let claim = claim_path(&dir, id);
        let result_path = result_path(&dir, id);
        std::fs::write(&request, "{}\n").unwrap();
        let create_calls = std::cell::Cell::new(0);
        let result = wait_result_with(
            &claim,
            Duration::ZERO,
            || {
                retract_request(&request)?;
                // Model the stale directory entry claiming only after removal.
                std::fs::write(
                    &claim,
                    serde_json::json!({"pid":std::process::id()}).to_string(),
                )
                .unwrap();
                let outcome = match std::fs::read(&request) {
                    Ok(_) => {
                        create_calls.set(create_calls.get() + 1);
                        serde_json::json!({"id":id,"ok":true,
                            "sessionId":"must-not-exist","agentPreset":"office-qa"})
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        serde_json::json!({"id":id,"ok":false,
                            "error":"dispatch request was cancelled"})
                    }
                    Err(e) => return Err(e.into()),
                };
                // The real plugin also writes its result before this removal.
                write_result(&dir, id, outcome);
                std::fs::remove_file(&claim).unwrap();
                Ok(())
            },
            || ready_result(&result_path, id),
        )
        .unwrap();
        assert!(!result.ok);
        assert_eq!(result.session_id, None);
        assert_eq!(create_calls.get(), 0);
        assert!(!request.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The result/claim release can straddle request retraction. Reading the
    /// result after the post-retraction claim check returns that real reply.
    #[test]
    fn a_result_arriving_during_retraction_is_returned() {
        let dir = scratch("result-during-retract");
        let id = "req-result-during-retract";
        let request = request_path(&dir, id);
        let result_path = result_path(&dir, id);
        std::fs::write(&request, "{}\n").unwrap();
        let result = wait_result_with(
            &claim_path(&dir, id),
            Duration::ZERO,
            || {
                retract_request(&request)?;
                write_result(
                    &dir,
                    id,
                    serde_json::json!({"id":id,"ok":true,
                        "sessionId":"session-during-retract","agentPreset":"office-qa"}),
                );
                Ok(())
            },
            || ready_result(&result_path, id),
        )
        .unwrap();
        assert_eq!(result.session_id.as_deref(), Some("session-during-retract"));
        assert!(!request.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A claim already observed before retraction is proof of acceptance even
    /// if it later disappears without a readable result. That stays unknown.
    #[test]
    fn a_seen_claim_that_disappears_remains_unknown_after_retraction() {
        let dir = scratch("seen-claim-disappears");
        let id = "req-seen-claim-disappears";
        let request = request_path(&dir, id);
        let claim = claim_path(&dir, id);
        std::fs::write(&request, "{}\n").unwrap();
        std::fs::write(
            &claim,
            serde_json::json!({"pid":std::process::id()}).to_string(),
        )
        .unwrap();
        let retracted = std::cell::Cell::new(false);
        let mut reads = 0;
        let error = wait_result_with(
            &claim,
            Duration::ZERO,
            || {
                retract_request(&request)?;
                retracted.set(true);
                Ok(())
            },
            || {
                reads += 1;
                if reads == 2 {
                    std::fs::remove_file(&claim).unwrap();
                }
                Ok(None)
            },
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("状态未知"), "{text}");
        assert!(!text.contains("尚未接受"), "{text}");
        assert!(retracted.get());
        assert!(!request.exists());
        assert_eq!(reads, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// If retraction fails after a Host result landed, that session id is the
    /// user's only recovery evidence. The dispatch must report an unknown
    /// outcome and leave both the request and result for the existing protocol.
    #[test]
    fn failed_retraction_preserves_a_landed_session_result() {
        let dir = scratch("failed-retraction-keeps-result");
        let id = "req-retract-error";
        let request_file = request_path(&dir, id);
        let result_file = result_path(&dir, id);
        let error = dispatch_in_with(
            &dir,
            request(id),
            Duration::ZERO,
            |result, id, request, claim, timeout| {
                wait_result_with(
                    claim,
                    timeout,
                    || {
                        // A real filesystem failure, not a synthetic anyhow error.
                        std::fs::remove_file(request).unwrap();
                        std::fs::create_dir(request).unwrap();
                        write_result(
                            &dir,
                            id,
                            serde_json::json!({"id":id,"ok":true,
                                "sessionId":"session-before-failed-retract",
                                "agentPreset":"office-qa"}),
                        );
                        retract_request(request)
                    },
                    || ready_result(result, id),
                )
            },
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(result_file.exists(), "result evidence was removed: {text}");
        assert!(request_file.is_dir(), "request path was removed: {text}");
        assert!(text.contains("状态未知"), "{text}");
        assert!(text.contains("先检查再重试"), "{text}");
        let preserved = ready_result(&result_file, id).unwrap().unwrap();
        assert_eq!(
            preserved.session_id.as_deref(),
            Some("session-before-failed-retract")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An accepted request whose Host dies is unknown, not a proven miss. The
    /// error asks for reconciliation rather than suggesting an immediate redo.
    #[test]
    fn a_host_dying_mid_dispatch_reports_an_unknown_outcome() {
        let dir = scratch("host-died");
        std::fs::write(claim_path(&dir, "req-dead"), r#"{"pid":2147483647}"#).unwrap();
        let error = dispatch_in(&dir, request("req-dead"), Duration::from_secs(1)).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("结果未知"), "{text}");
        assert!(text.contains("勿盲目重试"), "{text}");
        assert!(!request_path(&dir, "req-dead").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bad_input_never_creates_a_request() {
        let dir = scratch("bad-input");
        let mut bad = request("../escape");
        bad.role = "qa".into();
        assert!(dispatch_in(&dir, bad, Duration::ZERO).is_err());
        let mut bad = request("req-4");
        bad.role = "tester".into();
        assert!(dispatch_in(&dir, bad, Duration::ZERO).is_err());
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_stale_result_cannot_answer_a_reused_request_id() {
        let dir = scratch("stale");
        write_result(
            &dir,
            "req-5",
            serde_json::json!({
                "id":"req-5","ok":true,"sessionId":"old","agentPreset":"office-qa"
            }),
        );
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            wait_request(&worker_dir, "req-5");
            assert!(!result_path(&worker_dir, "req-5").exists());
            write_result(
                &worker_dir,
                "req-5",
                serde_json::json!({
                    "id":"req-5","ok":true,"sessionId":"new","agentPreset":"office-qa"
                }),
            );
        });
        let result = dispatch_in(&dir, request("req-5"), Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert_eq!(result.session_id, "new");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
