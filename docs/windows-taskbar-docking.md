# Windows compact-player taskbar docking

On Windows, drag a normal compact player close to a supported horizontal taskbar. A highlighted outline uses the customized Electron overlay's 240ms curve to travel into the taskbar; after that animation, only the taskbar strip is visible on the desktop. Clicking its upward arrow reverses the outline transition and then restores the compact player at its original size and position. Ordinary compact mode has no additional docking button. Reduced motion replaces geometric travel with a short fade.

## Window and playback ownership

The real main HWND stays top-level. The controller never reparents, reshapes or changes the decorations, resize policy or shadow of the real player. Its WebView retains the existing playback session while an independent `taskbar-lyrics` window provides the animation and attached content. Only that dedicated, transparent, borderless, non-resizable, shadowless window is parented into Explorer. It uses fixed 380 logical-pixel width and the selected taskbar's real height.

The overlay entry does not mount the main App or initialize Pinia, Player, migrations or account state. The existing player supplies bounded presentation snapshots; validated overlay actions return to that player. Thus progress, lyrics and play/shuffle/seek controls use the same playback session.

The attached appearance matches the customized Electron taskbar layout: a full-height square cover, 10-pixel title and artist on the top row, a 14-pixel single lyric below, four transport controls, separate return and close buttons, and a one-pixel progress line beginning after the cover. The background and native outer edges are transparent; the attached outer and client rectangles coincide.

## Behavior and recovery

Dock status is floating, docking, docked, undocking or waiting. Transitions block accidental compact expansion and stale frame persistence. The real player disappears from the desktop before the outline appears and returns only after the reverse animation finishes. An actual renderer-ready and visual-ready handshake prevents showing an uninitialized overlay.

Fullscreen or unreadable taskbar probes suppress the independent layer while preserving attachment intent. Single-instance or tray reopening during fullscreen keeps it suppressed. Recovery resumes only the taskbar layer without taking focus. Helper failures recover the dedicated overlay without rebuilding or changing the real player. Overlay close follows the original lyrics behavior: hide the visual layer while playback continues; an explicit reopen restores it.

Non-Windows builds keep their existing behavior. The helper is compiled and bundled only on Windows.

## Verification

Run Bun tests, full type checks and the production renderer build. For Rust changes, run workspace tests, Clippy and fmt. The local Windows environment has two known non-admin symbolic-link EPERM tests; report those explicitly rather than excluding them.

Native tests must identify the main HWND and the `taskbar-lyrics` HWND separately:

- Start with a valid floating compact frame (for example 900 by 150). Verify the main HWND remains parentless, its native styles and rectangle stay unchanged throughout both directions, and ordinary compact mode has no additional button.
- Sample actual native visibility during forward and reverse transitions. Main must be absent by the first visible outline frame and remain absent until the reverse outline is gone. Verify the original curve/duration and the reduced-motion variant.
- Verify only the dedicated overlay is attached to the selected taskbar at 380 DIP by its actual height, with coincident native outer/client rectangles and no non-client edge bits.
- Test on another supported monitor. Verify true hidden-player playback progress and matching snapshots, and actual play/shuffle/seek button delivery to the one existing player.
- Exercise rapid requests and verify stale transition generations cannot reattach or reveal a discarded layer.
- Make a known foreground fullscreen fixture active, then verify the taskbar layer stays hidden, including through reopening, and recovers without taking focus.
- Stop only this host's exact helper identified by its executable, host PID and overlay HWND; verify bounded independent-overlay recovery.
- Verify close/reopen intent, ordinary compact double-click expansion, and exit teardown. No taskbar HWND should remain owned by the exiting host.

State/geometry unit tests supplement these tests; they cannot establish native HWND behavior or animation visibility timing.
