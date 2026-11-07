import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import test from "node:test"
import vm from "node:vm"

const clientPath = process.env.AGENT_OFFICE_CLIENT
	? resolve(process.env.AGENT_OFFICE_CLIENT)
	: new URL("./client.js", import.meta.url)

function loadClient() {
	const registrations = []
	const window = {
		__ModuleLoader__: {
			load(definition) {
				registrations.push(definition)
			},
		},
	}
	const source = readFileSync(clientPath, "utf8")
	new vm.Script(source, { filename: String(clientPath) }).runInContext(
		vm.createContext({ window }),
	)
	assert.equal(registrations.length, 1)
	return { definition: registrations[0], client: registrations[0].factory(), window }
}

function row(id, options = {}) {
	return {
		id,
		origin: options.origin ?? "user",
		parentId: options.parentId,
		updatedAt: options.updatedAt ?? Date.now(),
		projectionValues: options.preset === null
			? {}
			: { agentPreset: options.preset ?? "office-qa" },
	}
}

function snapshot(rows, phase = "ready") {
	return {
		phase,
		ids: rows.map((item) => item.id),
		byId: Object.fromEntries(rows.map((item) => [item.id, item])),
	}
}

function apply(client, initialRows = []) {
	const listeners = new Set()
	const effects = []
	const opened = []
	const calls = { snapshots: 0, subscriptions: 0, effects: 0, cleanups: 0 }
	let state = snapshot(initialRows)
	const ctx = {
		sessions: {
			list: {
				getSnapshot() {
					calls.snapshots += 1
					return state
				},
				subscribe(listener) {
					calls.subscriptions += 1
					listeners.add(listener)
					return () => {
						calls.cleanups += 1
						listeners.delete(listener)
					}
				},
			},
		},
		uiWorkspace: {
			openSession(id) {
				opened.push(id)
			},
		},
		effect(callback) {
			calls.effects += 1
			const dispose = callback()
			if (typeof dispose === "function") effects.push(dispose)
		},
	}
	assert.equal(client.apply(ctx), undefined)
	return {
		opened,
		calls,
		publish(rows) {
			state = snapshot(rows)
			for (const listener of [...listeners]) listener()
		},
		unload() {
			for (const dispose of effects.splice(0).reverse()) dispose()
		},
	}
}

test("the client registers a loadable module without a dispatch bridge global", () => {
	const { definition, client, window } = loadClient()
	assert.equal(definition.id, "agent-office-dsh-web")
	assert.equal(client.name, "agent-office-web")
	assert.deepEqual(Array.from(client.inject), ["sessions", "uiWorkspace"])
	assert.equal(typeof client.apply, "function")
	assert.deepEqual(Object.keys(window), ["__ModuleLoader__"])
})

// A reload or a list reconnect must leave the session the user was reading alone.
test("an existing office session in the initial snapshot is not opened", () => {
	const { client } = loadClient()
	const page = apply(client, [row("already-there")])
	assert.deepEqual(page.opened, [])
})

// Time and an office preset cannot identify which page or client requested a Session.
test("a fresh office root created by another client never steals this page", () => {
	const { client } = loadClient()
	const current = row("current", { preset: "general", updatedAt: 1 })
	const page = apply(client, [current])
	page.publish([current, row("other-clients-dispatch")])
	assert.deepEqual(page.opened, [])
})

test("an office subagent remains in its parent view", () => {
	const { client } = loadClient()
	const page = apply(client)
	page.publish([row("child", { origin: "subagent", parentId: "parent" })])
	assert.deepEqual(page.opened, [])
})

// Metadata arriving later does not create an ownership relationship either.
test("an external row is not opened when its office projection arrives late", () => {
	const { client } = loadClient()
	const page = apply(client)
	const external = row("external", { preset: null })
	page.publish([external])
	page.publish([row("external")])
	assert.deepEqual(page.opened, [])
})

// Without an ownership contract apply must be inert, not retain a listener that
// can never make a safe decision. An inert module also has nothing to clean up.
test("apply has no list or workspace side effects and unload is inert", () => {
	const { client } = loadClient()
	const page = apply(client, [row("existing")])
	page.publish([row("existing"), row("another-page")])
	assert.deepEqual(page.opened, [])
	assert.deepEqual(page.calls, {
		snapshots: 0,
		subscriptions: 0,
		effects: 0,
		cleanups: 0,
	})
	assert.equal(page.unload(), undefined)
	assert.deepEqual(page.calls, {
		snapshots: 0,
		subscriptions: 0,
		effects: 0,
		cleanups: 0,
	})
})
