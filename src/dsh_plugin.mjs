// @agent-office-dsh-plugin — agent-office's cordis plugin for DeepSeek Harness.
// Written by `agent-office connect dsh`; edits are overwritten on reconnect.
// No @deepseek-ai imports: everything arrives through `ctx`, so this single
// file mounts from any absolute path.
//
// Four outputs, all best-effort:
// - The hook socket, through `agent-office hook --source dsh`: the payloads
//   pixtuoid-core's dsh decoder reads (ids, cwd, tool names, approvals), which
//   move the figures. Never prompts or tool arguments.
// - A local task log per session under TRANSCRIPTS, in the subset of Claude
//   Code's transcript shape the office's task reader parses, so one reader
//   serves both CLIs. It holds the human's prompts, the agent's replies and
//   each tool's target fields (path, command, pattern, url, todos), never file
//   contents or tool output; the office sends it to the summary API only when
//   one is configured.
// - A file per host under VIEWING: the ids of the sessions a dsh client shows
//   now, so the office can tell a reply you watched land from one unread.
//   Per host, since the app and a CLI can share one DSH_HOME.
// - A local file bridge under DISPATCH: agent-office asks this Host to create
//   a Session under an office-* preset, optionally submitting its first task.
const HOOK_EXE = __HOOK_EXE__
const TRANSCRIPTS = __TRANSCRIPTS__
const VIEWING = __VIEWING__
const DISPATCH = __DISPATCH__

import { spawn } from "node:child_process"
import {
  appendFileSync,
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  watch,
  writeFileSync,
} from "node:fs"
import { join } from "node:path"

export const name = "agent-office"
export const inject = ["agents"]

// The tool input fields the task reader shows or classifies by. Everything
// else (file contents, edit strings, subagent prompts) stays out of the log.
const KEEP_ARGS = ["file_path", "path", "command", "description", "pattern", "query", "url", "todos"]

// Every listener is synchronous, returns nothing and never throws, so this
// plugin cannot stall a tool call, a prompt, or shutdown.
export function apply(ctx) {
  // One shim at a time, in emit order: an approval decision that lands before
  // its ask would strand the figure waiting.
  let queue = Promise.resolve()
  const send = (payload) => {
    try {
      payload._pid = typeof process !== "undefined" ? process.pid : undefined
      const line = JSON.stringify(payload) + "\n"
      queue = queue.then(
        () =>
          new Promise((settled) => {
            try {
              const proc = spawn(HOOK_EXE, ["hook", "--source", "dsh"], {
                stdio: ["pipe", "ignore", "ignore"],
              })
              proc.on("error", settled)
              proc.on("close", settled)
              proc.stdin.on("error", () => {})
              proc.stdin.end(line)
            } catch {
              settled()
            }
          }),
      )
    } catch {}
  }

  // Sync appends keep lines in emit order; each is a few hundred bytes.
  let dirReady = false
  const log = (sessionId, record) => {
    try {
      if (!dirReady) {
        mkdirSync(TRANSCRIPTS, { recursive: true, mode: 0o700 })
        dirReady = true
      }
      const file = join(TRANSCRIPTS, sessionId.replace(/[^A-Za-z0-9._-]/gu, "_") + ".jsonl")
      appendFileSync(file, JSON.stringify(record) + "\n", { mode: 0o600 })
    } catch {}
  }

  // Only `origin: "subagent"` marks real delegation; a user branch stamps
  // `parentSession` too and must not render as a child.
  const base = (header) => ({
    sessionId: String(header.id),
    ...(header.cwd ? { cwd: header.cwd } : {}),
    ...(header.agentPreset ? { agentPreset: String(header.agentPreset) } : {}),
    ...(header.origin === "subagent" && header.parentSession
      ? { parentSession: String(header.parentSession) }
      : {}),
  })

  const started = new Set()
  const sendStart = (agent) => {
    try {
      const header = agent.session.header
      if (started.has(String(header.id))) return
      started.add(String(header.id))
      send({ type: "session_start", ...base(header) })
      if (header.agentPreset) {
        log(String(header.id), { type: "agent-office", agentPreset: String(header.agentPreset) })
      }
    } catch {}
  }
  const sendEnd = (agent) => {
    try {
      const header = agent.session.header
      if (!started.delete(String(header.id))) return
      send({ type: "session_end", ...base(header) })
    } catch {}
  }

  // `agent/created` is serial: dsh holds creation until it returns, and a
  // throw fails the agent's creation, so sendStart reads it inside its try.
  ctx.on("agent/created", (payload) => sendStart(payload?.agent))
  ctx.on("agent/disposed", ({ agent }) => sendEnd(agent))

  const slim = (raw) => {
    let args = raw
    if (typeof raw === "string") {
      try {
        args = JSON.parse(raw)
      } catch {
        return {}
      }
    }
    const out = {}
    if (!args || typeof args !== "object") return out
    for (const k of KEEP_ARGS) {
      if (k === "todos" && Array.isArray(args.todos)) {
        out.todos = args.todos.map((t) => ({ content: String(t?.content ?? ""), status: t?.status }))
      } else if (typeof args[k] === "string") {
        out[k] = args[k].slice(0, 300)
      }
    }
    return out
  }

  // approval/decided carries only {id, outcome}; the asked side owns
  // callId/toolName, so remember the pair until it resolves.
  const asked = new Map()
  // Calls the user turned down: their error result is not the agent failing.
  const rejected = new Set()
  ctx.on("session/event", (session, event) => {
    try {
      const header = session.header
      const sid = String(header.id)
      const d = event.data
      switch (event.type) {
        case "user/message":
          // Only the human's words: injected context (instructions, runtime
          // and time snapshots, skill catalogs) has its own source kind.
          if (d?.source?.kind === "user" && Array.isArray(d.content)) {
            const text = d.content
              .filter((b) => b?.type === "text" && typeof b.text === "string")
              .map((b) => b.text)
              .join(" ")
            if (text.trim())
              log(sid, {
                type: "user",
                // When the turn began, for the card's turn clock.
                timestamp: new Date().toISOString(),
                message: { content: text.slice(0, 4000) },
              })
          }
          break
        case "tool/call":
          send({
            type: "tool_call",
            ...base(header),
            ...(d.callId ? { callId: String(d.callId) } : {}),
            toolName: d.name,
          })
          log(sid, {
            type: "assistant",
            message: {
              content: [
                {
                  type: "tool_use",
                  ...(d.callId ? { id: String(d.callId) } : {}),
                  name: String(d.name ?? "?"),
                  input: slim(d.arguments),
                },
              ],
            },
          })
          break
        case "tool/result": {
          const callId = d.message?.toolCallId ? String(d.message.toolCallId) : undefined
          send({ type: "tool_result", ...base(header), ...(callId ? { callId } : {}) })
          const wasRejected = callId !== undefined && rejected.delete(callId)
          log(sid, {
            type: "user",
            message: {
              content: [
                {
                  type: "tool_result",
                  ...(callId ? { tool_use_id: callId } : {}),
                  is_error: d.message?.isError === true,
                  ...(wasRejected ? { rejected: true } : {}),
                },
              ],
            },
          })
          break
        }
        case "approval/asked":
          asked.set(String(d.id), { callId: d.callId, toolName: d.toolName })
          send({
            type: "approval_asked",
            ...base(header),
            ...(d.callId ? { callId: String(d.callId) } : {}),
            toolName: d.toolName,
            ...(d.reason ? { reason: d.reason } : {}),
          })
          break
        case "approval/decided": {
          const pair = asked.get(String(d.id))
          asked.delete(String(d.id))
          if (pair?.callId && d.outcome !== "allowed-once") rejected.add(String(pair.callId))
          send({
            type: "approval_decided",
            ...base(header),
            outcome: d.outcome,
            ...(pair?.callId ? { callId: String(pair.callId) } : {}),
            ...(pair?.toolName ? { toolName: pair.toolName } : {}),
          })
          break
        }
        case "assistant/message":
          // The reply's text, which the office reads a finished turn's outcome from.
          if (Array.isArray(d?.content)) {
            const text = d.content
              .filter((b) => b?.type === "text" && typeof b.text === "string")
              .map((b) => b.text)
              .join(" ")
            if (text.trim())
              log(sid, {
                type: "assistant",
                message: { content: [{ type: "text", text: text.slice(-3000) }] },
              })
          }
          break
        case "turn/start":
          // Opens the turn: the figure keeps its desk between calls.
          send({ type: "turn_start", ...base(header) })
          break
        case "turn/end":
          send({ type: "turn_end", ...base(header) })
          break
        default:
      }
    } catch {}
  })

  // A dsh client follows (`session/follow`) the one session in its main
  // view, and only that one: switching opens the new stream and closes the
  // old. No event says so, so the controller's follow is wrapped to count
  // the open streams per session. Not a listener: it yields what follow
  // yields, untouched, and the counting never throws into it. A dsh without
  // the controller, or with follow shaped otherwise, is left as it was.
  ctx.inject(["sessionController"], (ctx) => {
    const history = ctx.sessionController?.history
    const follow = history?.follow
    if (typeof follow !== "function") return
    const open = new Map()
    // Streams it wrapped can outlive it (a reload swaps the plugin under open
    // streams); their ends must not rewrite the file the new one keeps.
    let disposed = false
    // Named by pid, which the office also checks is still running: a crash
    // skips the cleanup below and must not leave a session viewed for good.
    const file = join(VIEWING, `${process.pid}.json`)
    const write = () => {
      if (disposed) return
      try {
        mkdirSync(VIEWING, { recursive: true, mode: 0o700 })
        const tmp = file + ".tmp"
        writeFileSync(tmp, JSON.stringify({ pid: process.pid, sessions: [...open.keys()] }), { mode: 0o600 })
        renameSync(tmp, file)
      } catch {}
    }
    const track = (id, delta) => {
      const n = (open.get(id) ?? 0) + delta
      if (n > 0) open.set(id, n)
      else open.delete(id)
      write()
    }
    const wrapped = async function* (request, signal) {
      let id
      try {
        const address = request?.address
        const raw = address?.childSessionId ?? address?.sessionId
        if (raw != null) {
          id = String(raw)
          track(id, 1)
        }
      } catch {}
      try {
        yield* follow.call(this, request, signal)
      } finally {
        if (id !== undefined) track(id, -1)
      }
    }
    history.follow = wrapped
    write()
    ctx.effect(() => () => {
      disposed = true
      // Another plugin may have wrapped ours since: theirs stays.
      if (history.follow === wrapped) history.follow = follow
      try {
        rmSync(file, { force: true })
      } catch {}
    })
  })

  // The dispatch bridge polls as well as watches: watchers can miss a rename,
  // and requests written while this Host was down are picked up on startup.
  // Requests run one at a time so a create and its optional first prompt stay
  // in order. Every failure is a result file, never an escaped rejection.
  ctx.inject(["sessionController"], (ctx) => {
    const roles = new Set([
      "product",
      "ux",
      "architect",
      "frontend",
      "backend",
      "qa",
      "devops",
      "reviewer",
      "pm",
    ])
    const queued = new Set()
    let dispatchQueue = Promise.resolve()
    let closed = false

    const writeResult = (id, result) => {
      const file = join(DISPATCH, `${id}.result.json`)
      const tmp = `${file}.${process.pid}.tmp`
      try {
        rmSync(tmp, { force: true })
        writeFileSync(tmp, JSON.stringify({ id, ...result }) + "\n", { mode: 0o600, flag: "wx" })
        renameSync(tmp, file)
        return true
      } catch {
        try {
          rmSync(tmp, { force: true })
        } catch {}
        return false
      }
    }

    // Only a result safely written to disk releases the claim. A failed write
    // leaves it in place: another Host must not execute the request again.
    const finish = (id, claim, result) => {
      if (!writeResult(id, result)) return
      try {
        rmSync(claim, { force: true })
      } catch {}
    }

    const process = async (name, claim) => {
      const id = name.slice(0, -".json".length)
      const path = join(DISPATCH, name)
      let createdIdentity
      try {
        if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(id) || id.endsWith(".result")) {
          throw new Error("invalid dispatch id")
        }
        const request = JSON.parse(readFileSync(path, "utf8"))
        if (!request || typeof request !== "object" || Array.isArray(request)) {
          throw new Error("dispatch request must be an object")
        }
        if (request.id !== id) throw new Error("dispatch id does not match its file name")
        if (!roles.has(request.role)) throw new Error("unknown dispatch role")
        if (typeof request.cwd !== "string" || !request.cwd.trim()) {
          throw new Error("dispatch cwd must be a non-empty string")
        }
        if (typeof request.prompt !== "string") throw new Error("dispatch prompt must be a string")
        if (typeof request.submit !== "boolean") throw new Error("dispatch submit must be a boolean")
        if (!Number.isFinite(request.expiresAt) || !Number.isSafeInteger(request.expiresAt)) {
          throw new Error("dispatch expiresAt must be a finite safe integer")
        }
        const ensureActive = () => {
          if (Date.now() >= request.expiresAt) throw new Error("dispatch request expired")
          if (!existsSync(path)) throw new Error("dispatch request was cancelled")
        }

        const agentPreset = `office-${request.role}`
        ensureActive()
        // A Session joins a sidebar Workspace only when created by its id; by
        // cwd alone it belongs to none, and is lost once you look away. The
        // registry reuses the directory's Workspace or registers one, as Add
        // workspace does. A Host without it still gets the Session by cwd.
        const registry = ctx.get("workspaceRegistry")
        const workspace = registry ? await registry.create(request.cwd) : undefined
        ensureActive()
        const created = await ctx.sessionController.create(
          workspace ? { workspaceId: workspace.id, agentPreset } : { cwd: request.cwd, agentPreset },
        ).catch((error) => {
          // A Session made but not attached to its Workspace still exists:
          // report it, or the user retries into a duplicate.
          const made = error?.details?.sessionId
          if (typeof made === "string" && made) createdIdentity = { sessionId: made, agentPreset }
          throw error
        })
        if (typeof created?.sessionId !== "string" || !created.sessionId) {
          throw new Error("DSH did not return a session id")
        }
        createdIdentity = {
          sessionId: created.sessionId,
          agentPreset: created.agentPreset ?? agentPreset,
        }
        if (request.submit) {
          // create has committed, but prompt must not follow an expired or
          // cancelled request. The error result keeps the created identity.
          ensureActive()
          await ctx.sessionController.prompt(
            {
              requestId: `agent-office-${id}`,
              sessionId: created.sessionId,
              mode: "queue",
              content: [{ type: "text", text: request.prompt }],
            },
            new AbortController().signal,
          )
        }
        finish(id, claim, { ok: true, ...createdIdentity })
      } catch (error) {
        let message = "unknown dispatch error"
        try {
          message = String(error instanceof Error ? error.message : error)
        } catch {}
        finish(id, claim, { ok: false, error: message.slice(0, 2000), ...createdIdentity })
      }
    }

    const scan = () => {
      if (closed) return
      try {
        for (const name of readdirSync(DISPATCH).sort()) {
          if (
            !name.endsWith(".json")
            || name.endsWith(".result.json")
            || name.endsWith(".claim.json")
            || queued.has(name)
          ) continue
          const id = name.slice(0, -".json".length)
          if (existsSync(join(DISPATCH, `${id}.result.json`))) continue
          // A pre-existing claim is never taken over, even if its owner died.
          // Request ids are fresh, so an old claim must not start a duplicate.
          queued.add(name)
          const claim = join(DISPATCH, `${id}.claim.json`)
          try {
            writeFileSync(claim, JSON.stringify({ pid: process.pid }) + "\n", {
              mode: 0o600,
              flag: "wx",
            })
          } catch {
            continue
          }
          dispatchQueue = dispatchQueue.then(() => process(name, claim)).catch(() => {})
        }
      } catch {}
    }

    let watcher
    try {
      mkdirSync(DISPATCH, { recursive: true, mode: 0o700 })
      chmodSync(DISPATCH, 0o700)
      watcher = watch(DISPATCH, scan)
      watcher.on("error", () => {})
      scan()
    } catch {}
    const timer = setInterval(scan, 250)
    timer.unref?.()
    ctx.effect(() => () => {
      closed = true
      clearInterval(timer)
      try {
        watcher?.close()
      } catch {}
    })
  })

  // The plugin's fiber may unwind before the agent registry's, so
  // agent/disposed is not guaranteed for every live agent: sweep them here.
  ctx.effect(() => () => {
    try {
      for (const agent of ctx.agents.list()) sendEnd(agent)
    } catch {}
  })
}
