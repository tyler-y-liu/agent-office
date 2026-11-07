import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { pathToFileURL } from "node:url"

// The process start marker is deliberately Darwin-only, just like the plugin.
const darwin = { skip: process.platform === "darwin" ? false : "requires the Darwin start marker" }

async function loadPlugin(t, follow, shared = {}) {
  const dir = shared.dir ?? mkdtempSync(join(tmpdir(), "agent-office-dsh-plugin-"))
  const viewing = join(dir, "viewing")
  const source = readFileSync(new URL("./dsh_plugin.mjs", import.meta.url), "utf8")
  const literal = (value) => JSON.stringify(value)
  const pluginPath = join(dir, "plugin.mjs")
  writeFileSync(
    pluginPath,
    source
      .replace("__HOOK_EXE__", literal(join(dir, "agent-office")))
      .replace("__TRANSCRIPTS__", literal(join(dir, "transcripts")))
      .replace("__VIEWING__", literal(viewing))
      .replace("__DISPATCH__", literal(join(dir, "dispatch"))),
  )
  const plugin = await import(pathToFileURL(pluginPath).href)
  const controller = shared.controller ?? { follow }
  const disposers = []
  const effect = (register) => {
    const dispose = register()
    if (typeof dispose === "function") disposers.push(dispose)
  }
  const injected = {
    sessionController: controller,
    effect,
    get() {
      return undefined
    },
  }
  plugin.apply({
    agents: { list: () => [] },
    on() {},
    effect,
    inject(services, callback) {
      assert.deepEqual(services, ["sessionController"])
      callback(injected)
    },
  })
  const file = join(viewing, `${process.pid}.json`)
  let disposed = false
  const dispose = () => {
    if (disposed) return
    disposed = true
    for (const cleanup of disposers.splice(0).reverse()) cleanup()
  }
  t.after(() => {
    dispose()
    rmSync(dir, { recursive: true, force: true })
  })
  return {
    controller,
    dir,
    originalFollow: follow,
    file,
    dispose,
    marker() {
      assert.equal(existsSync(file), true, `viewing marker was not created: ${file}`)
      return JSON.parse(readFileSync(file, "utf8"))
    },
    sessions() {
      return JSON.parse(readFileSync(file, "utf8")).sessions
    },
  }
}

function root(id) {
  return { address: { kind: "session", sessionId: id } }
}

function subagent(id) {
  return {
    address: {
      kind: "subagent",
      parentSessionId: "parent",
      childSessionId: id,
      mode: "continuable",
    },
  }
}

// Without a marker at apply time, desktop rejects the host even before the first view.
test("apply wraps direct follow and immediately writes a valid marker", darwin, async (t) => {
  const original = async function* () {}
  const env = await loadPlugin(t, original)
  assert.notEqual(env.controller.follow, original)
  const marker = env.marker()
  assert.equal(marker.pid, process.pid)
  assert.deepEqual(marker.sessions, [])
  const started = execFileSync("ps", ["-p", String(process.pid), "-o", "lstart="], {
    encoding: "utf8",
    env: { ...process.env, LC_ALL: "C" },
  }).trim()
  assert.equal(marker.startMarker, Math.floor(Date.parse(started) / 1000))
  assert.equal(statSync(env.file).mode & 0o777, 0o600)
  env.dispose()
  assert.equal(env.controller.follow, original)
  assert.equal(existsSync(env.file), false)
})

// A duplicate stream must not make closing one stream hide another live view.
test("follow preserves receiver and arguments while repeat roots close independently", darwin, async (t) => {
  const calls = []
  const firstFrame = { type: "first" }
  const secondFrame = { type: "second" }
  const original = async function* (request, signal) {
    calls.push({ receiver: this, request, signal })
    yield calls.length === 1 ? firstFrame : secondFrame
  }
  const env = await loadPlugin(t, original)
  env.marker()
  const request = root("root")
  const signal = new AbortController().signal
  const first = env.controller.follow(request, signal)
  const second = env.controller.follow(request, signal)
  assert.deepEqual(await first.next(), { value: firstFrame, done: false })
  assert.deepEqual(env.sessions(), ["root"])
  assert.deepEqual(await second.next(), { value: secondFrame, done: false })
  assert.deepEqual(env.sessions(), ["root"])
  assert.deepEqual(calls, [
    { receiver: env.controller, request, signal },
    { receiver: env.controller, request, signal },
  ])
  await first.return()
  assert.deepEqual(env.sessions(), ["root"])
  await second.return()
  assert.deepEqual(env.sessions(), [])
})

// Following a delegated child is not the same as switching the main root view.
test("subagent follow streams pass through without becoming viewed roots", darwin, async (t) => {
  const frame = { type: "child-frame" }
  const original = async function* () {
    yield frame
  }
  const env = await loadPlugin(t, original)
  const rootStream = env.controller.follow(root("root"), new AbortController().signal)
  await rootStream.next()
  assert.deepEqual(env.sessions(), ["root"])
  const childFrames = []
  for await (const value of env.controller.follow(subagent("child"), new AbortController().signal)) {
    childFrames.push(value)
    assert.deepEqual(env.sessions(), ["root"])
  }
  assert.deepEqual(childFrames, [frame])
  await rootStream.return()
  assert.deepEqual(env.sessions(), [])
})

// An old reload's open generator may finish after the replacement owns the same pid file.
test("an after-dispose stream cannot overwrite the replacement engine", darwin, async (t) => {
  const original = async function* (request) {
    yield { type: "snapshot", id: request.address.sessionId }
  }
  const oldEngine = await loadPlugin(t, original)
  const oldStream = oldEngine.controller.follow(root("old"), new AbortController().signal)
  await oldStream.next()
  assert.deepEqual(oldEngine.sessions(), ["old"])
  oldEngine.dispose()
  assert.equal(oldEngine.controller.follow, original)

  // The replacement shares the controller and pid file, as a plugin reload does.
  const shared = await loadPlugin(t, original, { dir: oldEngine.dir, controller: oldEngine.controller })
  assert.equal(shared.file, oldEngine.file)
  const newStream = shared.controller.follow(root("new"), new AbortController().signal)
  await newStream.next()
  assert.deepEqual(shared.sessions(), ["new"])
  await oldStream.return()
  assert.deepEqual(shared.sessions(), ["new"])
  await newStream.return()
  assert.deepEqual(shared.sessions(), [])
})
