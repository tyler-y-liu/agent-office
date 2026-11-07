import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import vm from "node:vm"

const source = readFileSync(new URL("./client.js", import.meta.url), "utf8")

// The Host's route, as the Host serves it: a change to one side without the
// other would leave every click opening nothing.
const JUMP_URL = "/api/agent-office/jump"

// The module's exports, loaded the way the page's loader does it.
function module() {
	const registrations = []
	const window = {
		__ModuleLoader__: {
			load(definition) {
				registrations.push(definition)
			},
		},
	}
	const context = vm.createContext({ window })
	new vm.Script(source, { filename: "client.js" }).runInContext(context)
	assert.equal(registrations.length, 1)
	return { definition: registrations[0], client: registrations[0].factory() }
}

// A page running the module's `apply`: each request is answered from
// `answers` in order (`{ sessionId }`, `{}`, or `{ fails: true }`), and its
// waits are captured so a test can move the poll on by one tick.
function page(answers = []) {
	const registrations = []
	const opened = []
	const fetches = []
	const timers = []
	const effects = []
	const context = vm.createContext({
		window: {
			__ModuleLoader__: {
				load(definition) {
					registrations.push(definition)
				},
			},
		},
		fetch: async (url, init) => {
			fetches.push({ url, init })
			const answer = answers.length > 0 ? answers.shift() : {}
			if (answer.fails) throw new Error("no answer")
			return { ok: true, json: async () => answer }
		},
		setTimeout: (callback) => {
			timers.push(callback)
		},
	})
	new vm.Script(source, { filename: "client.js" }).runInContext(context)
	assert.equal(registrations.length, 1)
	registrations[0].factory().apply({
		uiWorkspace: {
			openSession(id) {
				opened.push(id)
			},
		},
		effect(callback) {
			const dispose = callback()
			if (typeof dispose === "function") effects.push(dispose)
		},
	})
	return {
		opened,
		fetches,
		timers,
		unload() {
			for (const dispose of effects.splice(0).reverse()) dispose()
		},
	}
}

// Answer the request in flight, then let the page start the next one.
async function pump(timers) {
	for (let i = 0; i < 4; i += 1) await new Promise((resolve) => setImmediate(resolve))
	const next = timers.shift()
	if (next !== undefined) next()
	for (let i = 0; i < 4; i += 1) await new Promise((resolve) => setImmediate(resolve))
}

test("the page registers the workspace service the jump needs", () => {
	const { definition, client } = module()
	assert.equal(definition.id, "agent-office-dsh-web")
	assert.equal(client.name, "agent-office-web")
	assert.deepEqual(Array.from(client.inject), ["uiWorkspace"])
	assert.equal(typeof client.apply, "function")
})

// A click leaves one Session for the page: it is revealed as a search result
// would be, and the empty answers between clicks must reveal nothing.
test("a click's Session is revealed and an empty answer opens nothing", async () => {
	const page1 = page([{ sessionId: "session-a" }, {}, {}])
	await pump(page1.timers)
	assert.deepEqual(page1.opened, ["session-a"])
	assert.equal(page1.fetches[0].url, JUMP_URL)
	assert.equal(page1.fetches[0].init.cache, "no-store")

	await pump(page1.timers)
	await pump(page1.timers)
	assert.deepEqual(page1.opened, ["session-a"], "one click, one open")
	assert.equal(page1.fetches.length, 4, "the poll keeps asking")
})

// The Host may be restarting, or answer with no session at all; either way
// the page must keep asking rather than stop after one miss.
test("a failed request or a blank answer keeps the page polling", async () => {
	const page1 = page([{ fails: true }, { sessionId: "" }, { sessionId: "session-b" }])
	for (let i = 0; i < 3; i += 1) await pump(page1.timers)
	assert.deepEqual(page1.opened, ["session-b"])
	assert.equal(page1.fetches.length, 4)
})

// A reload replaces the plugin: the old loop must not keep asking, or it
// would race the new one for the next click.
test("unloading the page stops the poll", async () => {
	const page1 = page([])
	await pump(page1.timers)
	const asked = page1.fetches.length
	assert.ok(asked >= 2)
	page1.unload()
	await pump(page1.timers)
	assert.equal(page1.fetches.length, asked, "no request after unload")
})
