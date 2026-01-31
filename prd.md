[PRD]
# PRD: Agent Limits - Menubar/Tray + Settings Window + Alerts + History

## Overview
Add macOS menubar app + Windows tray app with minimal preview (CodexBar-style), split Settings into separate window with visual refresh, and add alerts + history/analytics. Data sources unchanged. Target: power users with many providers.

## Goals
- Menubar/tray preview shows current limits fast, multi-provider tabs
- Settings in dedicated window; refreshed visuals
- Alerts when limits near; opt-in, low noise
- Usage history/analytics visible without external tools

## Quality Gates
These commands must pass for every user story:
- `./scripts/clippy`
- `cargo test`

For UI stories, also include:
- Manual visual verification by user

Read `spec.md` for full details.

## User Stories

### US-001: Menubar/Tray App Shell
**Description:** As a power user, I want a menubar (macOS) / tray (Windows) app entry so I can access limits quickly.

**Acceptance Criteria:**
- [ ] macOS: app exposes menubar icon and can run without main window visible
- [ ] Windows: app exposes tray icon with toggle for preview window
- [ ] Main window can still be opened from menu/tray

### US-002: Minimal Preview UI (CodexBar-style)
**Description:** As a power user, I want a compact preview that mirrors CodexBar so I can scan limits quickly.

**Acceptance Criteria:**
- [ ] Preview uses multi-provider tabs for configured providers
- [ ] Each tab shows current limits in compact layout similar to reference image
- [ ] Tabs overflow/scroll when many providers configured
- [ ] No editing or settings controls inside preview (read-only)

### US-003: Preview Interactions -> Main Window
**Description:** As a power user, I want preview interactions to open the full app so I can act on details.

**Acceptance Criteria:**
- [ ] Clicking a provider tab or limit row opens main window (or focuses if already open)
- [ ] Main window opens on the selected provider context

### US-004: Separate Settings Window
**Description:** As a power user, I want Settings in a dedicated window so the main view stays focused.

**Acceptance Criteria:**
- [ ] Existing Settings UI moved out of main window into a new Settings window
- [ ] Settings window openable from app menu and from preview actions
- [ ] No settings UI remains in main window

### US-005: Settings Visual Refresh
**Description:** As a power user, I want a visually refreshed Settings window for faster scanning.

**Acceptance Criteria:**
- [ ] Settings layout updated for clearer sectioning and hierarchy
- [ ] Visual refresh does not change existing settings behavior
- [ ] Matches overall app style (fonts, spacing, card styles)

### US-006: Limit Alerts/Notifications
**Description:** As a power user, I want alerts when limits are near so I can avoid hitting caps.

**Acceptance Criteria:**
- [ ] Add alert thresholds (global default + per-provider override)
- [ ] Alert triggers when remaining percentage <= threshold for a limit window
- [ ] OS notification delivered (macOS + Windows)
- [ ] Cooldown prevents repeated notifications for same limit window

### US-007: History Capture
**Description:** As a power user, I want history stored so I can review past usage.

**Acceptance Criteria:**
- [ ] On each refresh, snapshot usage data for configured providers
- [ ] Store snapshots locally with retention window (configurable or default)
- [ ] No change to data-fetch sources

### US-008: History / Analytics View
**Description:** As a power user, I want a history view so I can see trends over time.

**Acceptance Criteria:**
- [ ] Add History view in main window or new view accessible from menu
- [ ] Show per-provider daily aggregates for a selectable range (default 30 days)
- [ ] Basic analytics: totals, averages, and deltas vs previous period

## Functional Requirements
- FR-1: Menubar/tray icons toggle the preview window
- FR-2: Preview window shows configured provider tabs and limit cards
- FR-3: Preview interactions open/focus main window with provider context
- FR-4: Settings moved to dedicated window; accessible from menu and preview
- FR-5: Settings visuals refreshed; no functional changes
- FR-6: Alerts configurable; OS notifications delivered with cooldown
- FR-7: History snapshots saved locally; retention enforced
- FR-8: History view exposes daily aggregates and summary stats
- FR-9: Auto + manual refresh behavior unchanged
- FR-10: Data sources remain unchanged

## Non-Goals
- New provider integrations
- Changes to existing data-fetch sources or auth flows
- Cloud sync or multi-device history
- Linux tray support (unless already present)

## Technical Considerations
- GPUI window management for preview + settings windows
- macOS menubar app behavior (dock visibility) vs Windows tray
- Local history storage format and size limits
- Notification throttling to avoid spam

## Success Metrics
- Preview opens in <1s from menubar/tray
- Users can access settings and history without main window clutter
- Alerts fire reliably and not more than once per cooldown
- No regressions in existing limit fetch/parsing

## Open Questions
- Default alert threshold value (e.g., 10% remaining?)
- Default history retention (30/90/180 days?)
- Should macOS dock icon be hidden when menubar mode enabled?
- Should history include cost, tokens, or both for all providers?
[/PRD]
