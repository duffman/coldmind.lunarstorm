<!-- OMX:RUNTIME:START -->
<session_context>
**Session:** omx-1793103088514-q35626 | 2026-10-27T12:11:33.244Z

**Native Subagent Routing:**
When the native surface exposes `agent_type` role routing, set `agent_type` to an installed OMX role and never omit it for OMX work.
On that routing-capable surface, use the most specific role (`architect`, `code-reviewer`, `critic`, `planner`, `debugger`, etc.); use `executor` only for generic implementation work.
When it reports `role_routing_unavailable` and adapted Ralplan authority is requested, do not fabricate `agent_type`; run `omx ralplan preflight --json` and stop on `unsupported_documented_leader_proof`. Ordinary work remains under its own workflow gates. Never fake the role via a prompt label or infer authority from session/thread/pointer/transcript/cwd state.

**Codebase Map:**
  src/: cminject.hot, cminject.inspect, cminject.react
  src/core/: cminject.container, cminject.contracts, cminject.core, cminject.decorators, cminject.errors, cminject.graph, cminject.token
  src/server/: cminject.context, cminject.runtime, cminject.server
  scripts/: bench, check-browser
  examples/: basic
  tests/: class-token.test, construct.test, lifetimes.test, regressions.test, decorators.test, decorators.test, container.test, adapter.test, setup
  (root): vitest.config, vitest.react.config

**Repository Lookup Routing:** use normal Codex repository inspection tools/subagents as the default surface for simple read-only repository lookup and implementation context.
- Use `omx sparkshell -- <command>` only for explicit shell-native read-only evidence or `--tmux-pane` summaries; it does not replace raw evidence capture.

**Compaction Protocol:**
Before context compaction, preserve critical state:
1. Write progress checkpoint via `omx state write --input '<json>' --json`
2. Save key decisions via `omx notepad write-working --input '<json>' --json`
3. Before large Team work near compaction, reload `.omx/state/team/<team>/preflight-context.json`
4. If context is >80% full, proactively checkpoint state
</session_context>
<!-- OMX:RUNTIME:END -->
