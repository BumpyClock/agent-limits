# Tray rich menu constraints (2026-02-01)

- macOS status item can show a menu, target-action, or a fully custom view; custom view is for the status item, not a rich menu UI. Source: Apple NSStatusBar docs.
- NSMenuItem supports attaching a custom NSView, but menu items then own all drawing and have limitations (no keyboard events; sizing/resize limits). Source: Apple "Views in Menu Items".
- Windows tray context menus are standard Win32 popup menus (TrackPopupMenu/TrackPopupMenuEx); rich UI is typically a separate flyout window, not the menu itself. Source: Microsoft TrackPopupMenuEx + NotificationIcon sample.
- Linux/GTK menu models (GMenuModel -> GtkMenu) do not support custom widgets in menu items; menus are limited to standard items. Source: Launchpad bug 1039476.

Implication: Rich CodexBar-like layout requires a custom popover/panel window (macOS NSPopover/NSPanel; Windows/Linus custom window) rather than a native tray menu.
