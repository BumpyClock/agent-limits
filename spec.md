# Agent Limits Desktop UX Spec

## Overview
- Scope: menubar/tray preview, separate Settings window + visual refresh, alerts, history/analytics
- Target: power users with many providers
- Data sources unchanged
- Reference: CodexBar preview UI

## Scope Tweaks (Review)
- Keep preview read-only; all actions route to main window
- Settings window is only place for configuration
- History as read-only analytics; no export in v1
- Alerts opt-in with per-provider override + cooldown
- macOS menubar + Windows tray only; Linux tray out-of-scope

## Inputs and Constraints
- Platforms: macOS + Windows desktop (GPUI)
- Windows: tray icon + toggle preview
- macOS: menubar icon + toggle preview; dock visibility TBD
- Content density: high; no layout shift
- Existing components: GPUI + gpui-component
- No data-source changes

## Information Architecture
- App entry points
  - Menubar/Tray icon
  - Main window
  - Settings window
- Views
  - Preview (compact limits)
  - Main (full limits + history entry)
  - History (inside main or separate tab)
  - Settings (dedicated window)

## Design Direction
- Personality: Precision & Density + Data & Analysis
- Foundation: cool neutral dark UI (CodexBar-aligned)
- Accent: single vivid blue for active tab and primary actions
- Depth: borders-only with subtle surface shifts

## Layout and Responsive Behavior
- Preview window (compact)
  - Fixed width; height grows to content with max + scroll
  - Tabs row + provider card stack
- Main window
  - Header + provider cards list + history entry
  - Scroll for provider list
- Settings window
  - Left nav (optional) + right content column
  - Sections: General, Providers, Alerts, History, About

## ASCII Layout
```text
Preview (menubar/tray)
+----------------------------------+
| Tabs: [Codex][Claude][Gemini]    |
+----------------------------------+
| Provider Card (compact)          |
| - Session   [bar] 93%            |
| - Weekly    [bar] 87%  +reserve  |
| - Sonnet    [bar] 96%            |
| Cost today / 30d                 |
+----------------------------------+
| Links: Add Account / Usage / ... |
+----------------------------------+

Main Window
+--------------------------------------------------+
| Header: Title + Refresh + Status                 |
+--------------------------------------------------+
| Provider Cards (scroll)                          |
| [Claude card.................]                   |
| [Gemini card.................]                   |
| [Copilot card................]                   |
+--------------------------------------------------+
| Footer: History entry / Open Settings            |
+--------------------------------------------------+

Settings Window
+--------------------------------------------------+
| Settings Title                                   |
+----------------------+---------------------------+
| Nav                  | Section Content           |
| - General            | [Cards/Forms]             |
| - Providers          |                           |
| - Alerts             |                           |
| - History            |                           |
| - About              |                           |
+----------------------+---------------------------+
```

## Component Inventory
- Menubar/Tray icon
  - States: idle, refreshing, error
- Preview window
  - Provider tabs (scrollable)
  - Compact limit rows with progress bars
  - Cost summary block
  - Quick links list
- Provider card (main)
  - Header with provider name + status
  - Limit windows (session/weekly/etc)
  - Cost summary
- Settings form blocks
  - Text inputs, toggles, dropdowns
  - Provider-specific config cards
- Alerts settings
  - Threshold slider or segmented options
  - Per-provider override
- History view
  - Range selector
  - Daily aggregate chart (sparkline or bars)
  - Summary stats grid

## Interaction and State Matrix
- Preview open/close
  - Menubar/tray click toggles preview
  - Escape closes preview
- Preview interactions
  - Click tab or limit row -> open/focus main window on provider
- Main window
  - Refresh button (manual)
  - Auto refresh unchanged
- Settings
  - Open from menu, preview, or main footer
  - Changes persist immediately
- Alerts
  - Trigger when remaining <= threshold
  - Cooldown per limit window
- States
  - Loading: skeletons or dimmed rows
  - Error: inline status + retry
  - Empty: explain no providers configured

## Motion Spec
- Open/close preview: 180ms ease-out; opacity + translateY(4px)
- Tab switch: 120ms ease (underline + text color)
- Refresh state: no animation on numeric changes; keep tabular-nums
- Main/Settings window open: OS-native; avoid custom animation
- Reduced motion: disable all transitions when OS reduce motion is enabled

## Visual System
- Color roles
  - Background: near-black cool gray
  - Surface: +2–4% lighter than background
  - Border: 10–12% white alpha
  - Text: primary 90% white, secondary 70%, muted 50%
  - Accent: single blue for active tab/primary action
  - Status: green (ok), amber (near limit), red (error)
- Typography
  - UI: platform default (SF Pro / Segoe UI)
  - Data: monospace for numbers; tabular-nums
  - Scale: 11/12/13/14/16/18
- Spacing
  - 4px grid; card padding 12–16
  - Compact rows 28–32px height
- Iconography
  - Simple line icons; consistent stroke

## Accessibility
- Keyboard nav: tabs -> limit rows -> links -> actions
- Focus ring: 2px accent outline, offset 2px
- Hit targets: 44px min for icon buttons
- Contrast: text >= 4.5:1 on surfaces
- Reduced motion honored

## Content Notes
- Copy tone: concise, technical
- Labels: “Session”, “Weekly”, “Resets in X”, “Lasts until reset”
- Errors: actionable (“Check API key”, “Re-authenticate”)

## Success Criteria
- Preview opens fast and remains readable at a glance
- Settings separated; main view uncluttered
- Alerts fire once per cooldown window
- History view useful without heavy charts
