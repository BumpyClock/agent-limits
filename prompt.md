# Current Progress (2026-01-28)

## Completed / In-progress (Provider Parity)
- ✅ Codex provider: implemented; lint warnings suppressed; `cargo check -p agent_stats_core` clean.
- ✅ Claude provider: parity fixes (UTF-8 safe truncation, CLI parsing safety, OAuth env token expiry), tests added.
- ✅ Gemini provider: parity fixes (primary window default, plan detection signature cleanup), tests updated.
- ✅ Antigravity provider: parity fixes (macOS guard, timeouts), request agent; tests ok.
- ✅ Copilot provider: token cleaning parity + device-flow dead-code annotations; tests added.
- ✅ Zai provider: implemented earlier.
- ✅ Synthetic provider: implemented earlier.

## Active Subagents (Batch 1)
- ⏳ Cursor provider (task-9) — reference analysis + implementation in progress.
- ⏳ Augment provider (task-10) — reference analysis in progress.
- ⏳ Amp provider (task-11) — reference analysis + implementation in progress.
- ⏳ Factory provider (task-12) — reference analysis in progress.
- ⏳ OpenCode provider (task-13) — implementation starting (new `opencode.rs`).

## Remaining Tasks

### Providers (Next Batch After Cursor/Augment/Amp/Factory/OpenCode)
- Kimi
- MiniMax
- Kiro
- VertexAI
- JetBrains
- KimiK2

### Integration / Wiring
- Add new provider modules to `crates/agent_stats_core/src/providers/mod.rs` registry when each file lands.
- Ensure any new deps (if needed) are added to workspace + `agent_stats_core`.
- Run `cargo check -p agent_stats_core` after each batch.
- Update `LEARNINGS.md` when notable changes land.

### App/UI Parity (after providers)
- Preferences UI for provider settings (cookie/API key/source modes)
- Refresh cadence settings and manual refresh
- Provider ordering (config-driven)
- Error/stale status handling UI
- Per-provider identity + reset description rendering

