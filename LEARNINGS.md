# Learnings

- Context: First GPUI compile for agent-stats UI.
  What I tried: Used `gpui::rgba(r,g,b,a)` and `.padding(...)`, and left `Render` returning `AnyElement` directly.
  Outcome: Build failed with gpui color API mismatch and missing `padding` (expects `.paddings`) plus Render signature mismatch.
  Next time: Use `gpui::rgba(0xRRGGBBAA)`/`gpui::rgb(0xRRGGBB)` and `.paddings(...)`, and keep Render signature as `impl IntoElement` with `AnyElement` wrapped.
