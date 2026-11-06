// @agent-office-dsh-web browser half — written by `agent-office connect dsh`.
// Plain script in DSH's client module format (no build step).
//
// A dispatch creates its Session on the Host, which no Host plugin can show:
// `dsh://open` only raises the window, and Host plugins cannot add events the
// page hears. So the page watches its own Session list and opens a Session
// that appears after it loaded under an office-* preset, the presets only
// `agent-office roles install --agent dsh` defines. Choosing such a preset
// in this page already opens the Session, so that case is unchanged.
window.__ModuleLoader__.load({
	id: "agent-office-dsh-web",
	factory: () => {
		var module = { exports: {} };
		const inject = ["sessions", "uiWorkspace"];
		function apply(ctx) {
			const list = ctx.sessions.list;
			// Everything the first loaded list holds predates this page: opening
			// one of those on reload or reconnect would steal the view.
			let seen = null;
			const check = () => {
				const state = list.getSnapshot();
				if (state.phase === "pending") return;
				if (seen === null) {
					seen = new Set(state.ids);
					return;
				}
				let open;
				for (const id of state.ids) {
					if (seen.has(id)) continue;
					const row = state.byId[id];
					const preset = row?.projectionValues?.agentPreset;
					// The preset can land after the row; decide once it is known.
					if (typeof preset !== "string") continue;
					seen.add(id);
					// A subagent belongs to its parent's view, not a dispatch.
					if (preset.startsWith("office-") && row.origin !== "subagent" && row.parentId === undefined) open = id;
				}
				if (open !== undefined) ctx.uiWorkspace.openSession(open);
			};
			ctx.effect(() => list.subscribe(check));
			check();
		}
		module.exports.name = "agent-office-web";
		module.exports.inject = inject;
		module.exports.apply = apply;
		return module.exports;
	},
});
