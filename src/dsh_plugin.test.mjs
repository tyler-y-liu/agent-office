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
  const jump = join(dir, "jump.json")
  const source = readFileSync(new URL("./dsh_plugin.mjs", import.meta.url), "utf8")
  const literal = (value) => JSON.stringify(value)
  const pluginPath = join(dir, "plugin.mjs")
  writeFileSync(
    pluginPath,
    source
      .replace("__HOOK_EXE__", literal(join(dir, "agent-office")))
      .replace("__TRANSCRIPTS__", literal(join(dir, "transcripts")))
      .replace("__VIEWING__", literal(viewing))
      .replace("__DISPATCH__", literal(join(dir, "dispatch")))
      .replace("__JUMP__", literal(jump)),
  )
  const plugin = await import(pathToFileURL(pluginPath).href)
  const controller = shared.controller ?? { follow }
  const disposers = []
  const effect = (register) => {
    const dispose = register()
    if (typeof dispose === "function") disposers.push(dispose)
  }
  const routes = new Map()
  const injected = {
    sessionController: controller,
    connection: {
      fetch: {
        register(route) {
          routes.set(route.path, route)
          return () => routes.delete(route.path)
        },
      },
    },
    effect,
    get() {
      return undefined
    },
  }
  const services = []
  plugin.apply({
    agents: { list: () => [] },
    on() {},
    effect,
    inject(requested, callback) {
      services.push(requested)
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
    jump,
    routes,
    services,
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
  assert.deepEqual(env.services, [["sessionController"], ["connection"], ["sessionController"]], "viewing, jump, dispatch")
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

// A click in agent-office names one Session for this Host's page. The page
// takes it once: a click must open exactly one window, and reading it must
// not leave a request that opens the same Session again on the next poll.
test("a click's Session is served once to the Host it names", darwin, async (t) => {
  const env = await loadPlugin(t, async function* () {})
  const marker = env.marker().startMarker
  const route = env.routes.get("/api/agent-office/jump")
  assert.deepEqual(route.methods, ["GET"])
  const ask = async () => (await route.fetch({})).json()

  assert.deepEqual(await ask(), {}, "no click yet")
  writeFileSync(
    env.jump,
    JSON.stringify({
      sessionId: "session-a",
      pid: process.pid,
      startMarker: marker,
      expiresAt: Date.now() + 60_000,
    }),
  )
  assert.deepEqual(await ask(), { sessionId: "session-a" })
  assert.equal(existsSync(env.jump), false, "a served click is consumed")
  assert.deepEqual(await ask(), {}, "a click opens one page, once")
})

// A desktop app and a `dsh web` share DSH_HOME, and a crash can hand a pid to
// another process: only the Host the click names may open it.
test("a click for another Host or an expired one is never opened here", darwin, async (t) => {
  const env = await loadPlugin(t, async function* () {})
  const marker = env.marker().startMarker
  const route = env.routes.get("/api/agent-office/jump")
  const ask = async () => (await route.fetch({})).json()
  const leave = (sessionId, pid, startMarker, expiresAt) =>
    writeFileSync(env.jump, JSON.stringify({ sessionId, pid, startMarker, expiresAt }))

  leave("their-page", process.pid + 1, marker, Date.now() + 60_000)
  assert.deepEqual(await ask(), {})
  assert.equal(existsSync(env.jump), true, "left for the Host it names")

  leave("reused-pid", process.pid, marker + 1, Date.now() + 60_000)
  assert.deepEqual(await ask(), {}, "another incarnation of this pid")

  // A Host that cannot name its own start (no kernel-equivalent marker)
  // must not take a click on its pid alone.
  writeFileSync(env.jump, JSON.stringify({ sessionId: "no-marker", pid: process.pid, expiresAt: Date.now() + 60_000 }))
  assert.deepEqual(await ask(), {}, "a pid alone is not an incarnation")

  leave("too-late", process.pid, marker, Date.now() - 1)
  assert.deepEqual(await ask(), {}, "a page opened later must not jump")
  assert.equal(existsSync(env.jump), false, "an expired click is dropped")

  writeFileSync(env.jump, "{")
  assert.deepEqual(await ask(), {}, "a half-written click says nothing")
  assert.equal(existsSync(env.jump), false)
})
