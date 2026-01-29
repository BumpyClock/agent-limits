# Learnings

- Context: First GPUI compile for agent-stats UI.
  What I tried: Used `gpui::rgba(r,g,b,a)` and `.padding(...)`, and left `Render` returning `AnyElement` directly.
  Outcome: Build failed with gpui color API mismatch and missing `padding` (expects `.paddings`) plus Render signature mismatch.
  Next time: Use `gpui::rgba(0xRRGGBBAA)`/`gpui::rgb(0xRRGGBB)` and `.paddings(...)`, and keep Render signature as `impl IntoElement` with `AnyElement` wrapped.
- Context: Started Rust provider porting with Synthetic.
  What I tried: Implemented a native provider registry + Synthetic API fetcher with tolerant JSON parsing and API key lookup.
  Outcome: Works with `SYNTHETIC_API_KEY` or config `apiKey`, returns native payloads without codexbar CLI.
  Next time: Prefer blocking HTTP (ureq) on background threads to avoid async runtime conflicts with GPUI.
- Context: Ported Copilot/Gemini/Antigravity/Claude provider parity updates.
  What I tried: Matched Swift reference behavior (token cleaning, Gemini primary window defaults, Antigravity macOS guard + timeouts, Claude env token expiry logic) and removed dead-code warnings.
  Outcome: `cargo check -p agent_stats_core` is clean with no warnings.
  Next time: If a helper is test-only, gate it with `#[cfg(test)]` to avoid dead-code warnings.
- Context: Adding Claude Web API (cookie-based) strategy porting from CodexBar Swift reference.
  What I tried: Implemented all four claude.ai API endpoints (organizations, usage, overage_spend_limit, account) using ureq with session cookie auth, matching the Swift reference behavior including organization capability filtering, plan inference from rate_limit_tier/billing_type, and extra usage cost normalization.
  Outcome: Works; 29 tests pass. Found pre-existing bugs: type mismatch in OAuth tertiary window selection (`Option<&T>` vs `&Option<T>`) and wrong expected value in `find_matching_brace` test.
  Next time: The claude.ai web API returns utilization as either integer or float depending on the endpoint version; parse both. Organization selection should prefer "chat" capability, then non-API-only, then first.
- Context: Ported Cursor/Augment/Amp/Factory/OpenCode providers from CodexBar.
  What I tried: Implemented cookie-header web fetchers with ureq, HTML parsing for Amp, and OpenCode JSON candidate scanning; tightened structs to avoid dead-code warnings.
  Outcome: `cargo check -p agent_stats_core` clean; providers compile without warnings.
  Next time: Watch Rust raw string regex escaping and normalize millisecond epoch timestamps before converting.
- Context: Ported Kimi, MiniMax, Kiro, VertexAI, JetBrains, and Kimi K2 providers.
  What I tried: Implemented cookie/JWT/API fetchers, HTML/Next.js parsing for MiniMax, CLI parsing for Kiro, and OAuth refresh for Vertex AI.
  Outcome: `cargo check -p agent_stats_core` clean; provider registry updated.
  Next time: Avoid raw-string regex escaping pitfalls by preferring `r#"..."#` or standard strings.
- Context: Added provider settings UI and identity/stale indicators in GPUI app.
  What I tried: Used gpui-component InputState + Input with masked toggles, per-provider settings spec mapping, and save-to-config refresh helpers; added reset formatting tests.
  Outcome: Settings UI renders provider-specific fields and persists config updates; reset text avoids double "Resets" prefix.
  Next time: Keep provider settings spec in sync with provider capabilities and update InputState defaults when reloading config.
- Context: Added token account UI actions in GPUI app state.
  What I tried: Read InputState from token_account_forms and then called persist_config_and_refresh in the same scope.
  Outcome: Borrow checker error E0502 due to immutable borrow of forms across mutable app update.
  Next time: Clone InputState entities and values before mutating app state or saving config.
- Context: Claude CLI usage probe failed when app launched from GUI.
  What I tried: Added login-shell discovery for claude (`/bin/zsh -lc 'command -v claude'`), login-shell PATH injection, and common install path fallbacks; added PATH/CLAUDE_BINARY diagnostics in CLI errors.
  Outcome: CLI resolution is resilient to GUI PATH issues; `cargo check -p agent_stats_core` passes.
  Next time: Prefer login-shell PATH discovery when invoking CLI tools from GUI apps.
- Context: Added provider ordering controls and config reorder helper.
  What I tried: Added Up/Down buttons in settings, implemented safe config reordering with saturating indices, and covered move cases with unit tests.
  Outcome: `cargo test -p agent-stats move_provider` passes; provider order persists and drives both lists.
  Next time: Rebuild provider state from config after reorder to keep UI and data in sync.
- Context: Providers hidden below the fold and window chrome felt incomplete.
  What I tried: Added vertical scrolling to main/settings containers and set explicit centered window bounds with a standard title.
  Outcome: UI can scroll to all providers and window opens at a predictable size.
  Next time: Add a custom TitleBar only when rendering its view; otherwise prefer default window chrome.
- Context: Claude CLI usage probe failed with "unknown option '--no input'".
  What I tried: Added CLI fallback to retry `/usage` without `--no-input` when the CLI reports unknown option.
  Outcome: Older/newer CLI versions should both work; tests cover the detection helper.
  Next time: Match CodexBar’s PTY-based Claude probe if more CLI incompatibilities appear.
- Context: Claude CLI `/usage` needed PTY parity with CodexBar.
  What I tried: Added portable-pty based capture, prompt auto-responders, and fallback for older `--allowed-tools` support.
  Outcome: CLI usage now runs in a PTY like CodexBar; tests cover option detection.
  Next time: If parsing still fails, expand stop/idle heuristics to match Swift probe timing.
- Context: Strict Claude CLI parity requested.
  What I tried: Removed non-PTY fallback path so CLI usage always uses portable-pty capture.
  Outcome: CLI probing is PTY-only, matching CodexBar behavior.
  Next time: If PTY fails on some setups, document environment prerequisites instead of falling back.
- Context: Claude CLI parse failed despite expected labels in output.
  What I tried: Matched CodexBar PTY behavior by handling cursor position queries and removing idle-time termination for `/usage`.
  Outcome: PTY capture aligns with Swift probe; added a test for the current Claude CLI output format.
  Next time: If output changes again, update stop/settle heuristics and add a fixture test first.
- Context: Claude CLI still missing expected labels after PTY capture.
  What I tried: Added PTY debug dump and stop-reason logging to capture raw CLI output for troubleshooting.
  Outcome: Enabling `AGENT_STATS_CLAUDE_PTY_DEBUG=1` writes the raw output to ~/.agent-stats/claude-cli-debug.txt.
  Next time: Use the debug dump before altering parsing heuristics.
- Context: PTY debug dump showed Claude CLI stuck at welcome prompt and timing out.
  What I tried: Added a 400ms startup delay and drained initial PTY output before sending `/usage`, matching CodexBar’s session warmup.
  Outcome: PTY probe should now issue `/usage` after the prompt is ready.
  Next time: If still stuck, detect prompt glyphs before sending command.
- Context: Claude PTY output showed /usage echoed but UI stayed on welcome screen.
  What I tried: Waited for prompt glyphs before sending /usage, then drained pre-prompt output.
  Outcome: /usage should now send only after the interactive prompt is ready.
  Next time: Add a stricter prompt detector if Claude UI changes again.
- Context: Wired Copilot GitHub OAuth device flow into the GPUI settings UI.
  What I tried: Re-exported existing device flow functions from agent_stats_core, added state enum and async methods on AgentStatsApp, rendered inline UI in the Copilot settings card with clipboard/browser integration.
  Outcome: Compiles clean; all 189 tests pass. Device flow lifecycle: Idle -> RequestingCode -> ShowingCode -> Polling -> Success/Error.
  Next time: Closures passed to `cx.spawn` that capture non-Copy locals need `move`; form rebuild after config change requires a flag checked in `render()` since `window` is only available there.
- Context: Claude CLI PTY `/usage` capture was inconsistent.
  What I tried: Reworked PTY capture to mirror CodexBar’s session loop, enriched CLI environment defaults, and added `/status` fallback for identity parsing.
  Outcome: PTY capture follows CodexBar timing and prompt handling; `cargo check -p agent_stats_core` clean.
  Next time: Keep the PTY loop aligned with the Swift reference before adding new heuristics.
- Context: Claude CLI `/usage` input was being sent before the prompt was ready.
  What I tried: Added a prompt-ready wait (up to 6s) before sending `/usage`, then drained output to avoid pre-init noise.
  Outcome: PTY capture should no longer send `/usage` before the UI is ready.
  Next time: If the prompt wait still races, confirm cursor query handling before adding longer delays.
- Context: Claude Sonnet weekly limit needed to show separately.
  What I tried: Rendered tertiary usage windows in the UI and labeled Claude's tertiary window as "Sonnet".
  Outcome: Claude cards now show the extra Sonnet limit when available.
  Next time: Consider provider metadata labels if more providers expose tertiary windows.
- Context: Claude CLI PTY capture loop timed out because stop needles (e.g. "Current session") were fragmented by ANSI cursor-positioning escape sequences in raw PTY bytes.
  What I tried: Added `strip_ansi_bytes()` to strip CSI/OSC/Fe sequences from raw bytes, and added a secondary ANSI-stripped fallback check in the capture loop alongside the existing raw rolling-buffer check. Also enhanced `strip_ansi_codes()` with OSC sequence handling.
  Outcome: All 275 tests pass including new tests for byte-level stripping and fragmented needle matching; no new clippy warnings.
  Next time: When matching text needles in PTY output, always consider that TUI apps use cursor-positioning escapes that fragment rendered text in the raw byte stream.
- Context: Claude CLI PTY `/usage` sent before CLI was ready — prompt needle `❯` appeared ~1s during TUI init but rendering continued for hundreds of milliseconds more.
  What I tried: Replaced immediate `break` on prompt needle detection with a two-phase approach: (1) detect needle as before, (2) settle phase — wait for 500ms of PTY idle or 3s max after needle detection before proceeding. Also added ANSI-stripped fallback for send_needles during prompt wait.
  Outcome: All 275 tests pass; `/usage` is now sent only after the TUI finishes rendering, not just after the first prompt glyph appears.
  Next time: When a TUI prompt character appears early in initialization, don't assume the TUI is ready — wait for output to settle (idle gap) before sending input.
