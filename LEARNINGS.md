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
