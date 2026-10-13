# Independent Windows taskbar overlay

The user reported that reparenting the real compact player sometimes exposes native window chrome. Replace that architecture with the customized Electron desktop-lyrics pattern: a dedicated transparent lyrics window is the only window attached to Explorer. After the snap animation, the desktop shows only the taskbar strip. Returning from the strip plays the reverse outline animation before the compact player appears again.

## Approved behavior

- A real, normal compact main window is required for automatic snapping. Ordinary compact mode gains no additional buttons.
- The main player keeps its one playback session. Its HWND remains top-level; docking never changes its parent, resize policy, shadow, bounds or non-client style. Native visibility may change while its WebView continues playback.
- A lightweight `taskbar-lyrics` WebView loads `#/taskbar-overlay` without the main App, Pinia, Player, migrations or account initialization. It reuses the exact customized Electron taskbar layout in TaskbarPlayer.
- Main publishes only artwork, track metadata, current lyric, progress, playback/shuffle state, labels, locale and reduced-motion preference. Overlay actions are validated and delivered once to the existing player. No second player or account state is created.
- An independent highlight outline replaces the compact window at snap start. The real compact window disappears from the desktop during the animation. The outline follows the original 240ms cubic-bezier(.77,0,.175,1) rectangle transition. Only after completion is the overlay attached to the taskbar and its controls shown.
- Return reverses that path. Main stays absent throughout the animation, then appears at its unchanged compact frame. Reduced motion keeps a short fade and avoids geometric travel.
- Fullscreen and unreadable native probes hide the visual layer while retaining intent. Recovery does not activate another window. Helper failure rebuilds/recoveries affect only the overlay.
- Close on the overlay follows the original lyrics behavior: hide visual lyrics while preserving playback; tray or second-instance reopen restores the requested visual mode.

## Integration contract

Dock status modes: floating, docking, docked, undocking, waiting. Existing status fields remain mode/suspended/dockError/enabled.

Native commands:

- `publish_taskbar_overlay_snapshot({snapshot})`: main-only, bounded validated presentation DTO.
- `taskbar_overlay_ready()`: overlay-only; returns `{snapshot, visual}` and marks the actual overlay renderer ready.
- `taskbar_overlay_action({action,value?})`: overlay-only; previous/play/next/shuffle/seek relay to main; undock and close are handled by the controller.

Events:

- `desktop://taskbarOverlaySnapshot`: presentation DTO sent only to overlay.
- `desktop://taskbarOverlayVisual`: `{phase:hidden|snapping|docked|returning,token}`; generation prevents stale completion.
- `desktop://taskbarOverlayAction`: validated playback actions sent only to main.

## Implementation and verification plan

1. Implement the lightweight entry, pure DTO/action validation, single main publisher and reused taskbar view. Keep ordinary main compact rendering intact.
2. Split the native controller into main-window eligibility/intent and independent-overlay lifecycle, helper attachment and forward/reverse animation. Add overlay-scoped event permissions and command label checks.
3. Run meaningful schema/state/transition tests, full Bun/typecheck/renderer checks and Rust workspace/Clippy/fmt checks. Report the existing Windows symlink EPERM checks separately.
4. Build a protocol runtime and verify both HWNDs independently: main parent/style/frame invariants throughout every animation, exact overlay taskbar geometry, button-to-player delivery, true hidden playback progress, rapid reversal, fullscreen, helper death and reopen.
5. Integrate into the three-feature branch, preserve QQ and compact expansion, back up and replace the installed local portable build, and verify the desktop shortcut still starts it.
6. Push the reviewed taskbar branch and update existing PR #38 and its final description; create no additional PR and merge no upstream PR.

## Docking feedback and recovery

Dragging or shrinking the compact player near a horizontal taskbar publishes a
bounded docking hint. A ready hint shows a 2px white outline at the visible
window edge, with no inset or independent rounded frame. Only its highlight
texture rotates, over eight seconds; idle state has no animation, and reduced
motion keeps the static white outline. The mask fixes all four edge widths.

Forward and reverse overlay flights follow the native window geometry. A
foreground window change requires a fresh fullscreen/taskbar probe before a
visual layer can be shown; failed or unreadable probes retain hidden recovery
intent. The return path rechecks safety before restoring the compact player.

WebView2 taskbar-child surfaces use non-draggable app regions. The window-state
plugin excludes the overlay so persisted visibility cannot restore a cached
caption style, and the helper redraws an exposed caption residue beneath a
restyled child.

Renderer HTML documents are served with no-store and ignore conditional
validators. This prevents a stale index from referencing removed hashed assets
after switching builds; hashed assets retain conditional cache validation.

When the strip is already attached, a complete clear probe may tolerate pure
position changes of the same valid foreground HWND. Both requested-visibility
checks must still be true, and at least one window dimension must be smaller
than that probe's monitor, so the moving window cannot cover the entire monitor.
Size or HWND changes, unreadable samples, transitions and potentially
fullscreen-sized windows retain the strict retry path. A fresh safety probe
still runs before showing the attached overlay.
