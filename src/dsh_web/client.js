// @agent-office-dsh-web browser half — written by `agent-office connect dsh`.
// Plain script in DSH's client module format (no build step).
//
// The Host creates dispatch Sessions outside this page. Its create call has no
// dispatch request id, and the session id returned to agent-office never reaches
// this client, so a Session-list row cannot identify this page's dispatch.
// Opening a fresh office-* row would let another client's dispatch steal this
// page's view; the user can instead open the Session named in the dispatch result.
window.__ModuleLoader__.load({
	id: "agent-office-dsh-web",
	factory: () => {
		var module = { exports: {} };
		const inject = ["sessions", "uiWorkspace"];
		// Sessions created by this page already follow DSH's existing UI flow.
		// Do not retain a list listener that has no safe automatic choice to make.
		function apply() {}
		module.exports.name = "agent-office-web";
		module.exports.inject = inject;
		module.exports.apply = apply;
		return module.exports;
	},
});
