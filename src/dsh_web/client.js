// @agent-office-dsh-web browser half — written by `agent-office connect dsh`.
// Plain script in DSH's client module format (no build step).
//
// A click in agent-office names the Session to show. DSH has no deep link to
// one, and the Host cannot switch this page's view, so the office leaves the
// id for its Host and this page asks for it: the Host's route answers with it
// once, and `uiWorkspace.openSession` reveals it as a search result would.
window.__ModuleLoader__.load({
	id: "agent-office-dsh-web",
	factory: () => {
		var module = { exports: {} };
		const inject = ["uiWorkspace"];
		// The Host's route for one click's Session; local, so a tick a second
		// costs nothing and a jump lands as the window comes forward.
		const JUMP_URL = "/api/agent-office/jump";
		const EVERY_MS = 1000;
		function apply(ctx) {
			let disposed = false;
			ctx.effect(() => () => {
				disposed = true;
			});
			// One request at a time: the Host consumes each answer, and two
			// requests in flight would race this page for the next click.
			const loop = async () => {
				while (!disposed) {
					let sessionId;
					try {
						const response = await fetch(JUMP_URL, {
							cache: "no-store",
							headers: { accept: "application/json" },
						});
						if (response.ok) {
							const jump = await response.json();
							if (jump && typeof jump.sessionId === "string" && jump.sessionId) {
								sessionId = jump.sessionId;
							}
						}
					} catch {}
					if (disposed) return;
					if (sessionId !== undefined) {
						try {
							ctx.uiWorkspace.openSession(sessionId);
						} catch {}
					}
					await new Promise((resolve) => setTimeout(resolve, EVERY_MS));
				}
			};
			loop();
		}
		module.exports.name = "agent-office-web";
		module.exports.inject = inject;
		module.exports.apply = apply;
		return module.exports;
	},
});
