# Windows taskbar docking for the compact player

This feature adds taskbar attachment to the existing compact main-window player. It does not create another lyrics window or replace its player interface. Enter the existing compact mode by reducing the main window's height below 340 logical pixels while it is neither maximized nor fullscreen. The native controller validates the current window state before attaching; a renderer compact-mode flag cannot bypass this check.

On Windows, move the compact player close to a supported horizontal taskbar to attach it automatically. A floating compact player has no docking button. The dock uses a fixed 380 logical-pixel width and the taskbar's actual height. The attached layout copies the customized Electron taskbar overlay: a full-height square cover, 10-pixel title and artist on the top row, a 14-pixel single lyric line below, four transport controls, separate return and close buttons, and a one-pixel progress line beginning after the cover. Its background is transparent and has no frame or rounded corners. While docked, native non-client edge bits, Tao's undecorated shadow insets, and its cached resize overlay are disabled. Detach restores the saved floating frame and resize state without activating or raising the window. Once attached, the upward-arrow button returns it to the saved floating compact frame. Double-clicking the attached player does not expand it into the main player or queue. Once floating again, its existing double-click expansion behavior is retained.

While attached, fullscreen foreground windows on the same monitor hide the compact player automatically. Reopening during fullscreen retains the dock request and stays hidden; the player recovers without taking focus after fullscreen ends. Missing or unreadable native probes retain that intent and stay hidden. Explicitly hiding or closing the player remains respected.

All attachment, native probing and helper startup are Windows-only. macOS and Linux retain the existing compact-window behavior.

## Building and verification

Use the normal Windows Tauri build. The included C# source is compiled by the Windows .NET Framework compiler and bundled only for Windows. Its STA helper uses documented Win32 APIs and the taskbar's accessibility interface; it does not inject code into Explorer.

Run the repository's Bun tests, type checks and renderer build, plus the Rust workspace tests, clippy and formatting checks. Native runtime verification additionally checks actual HWND parenting and geometry, fullscreen hide/recovery, explicit hide, and restoration before expanding or exiting. Geometry/state unit tests do not replace native Windows verification.

## Native runtime checklist

Use a separate WebView2 user-data directory and skip legacy Electron import in the test profile. Run one host/sidecar at a time because the application uses fixed loopback ports. Keep the installed Electron application and other test sessions untouched.

- Start with a legal floating compact frame such as 900 by 150 logical pixels at an arbitrary screen position. Move it near a supported horizontal taskbar and wait for automatic attachment. Verify that the floating state has no docking button and that the return-to-floating arrow appears only after docking. Inspect the same main HWND: its parent must be the selected taskbar, its content width must be 380 logical pixels, and its height must fit the real taskbar. Its native outer and client rectangles must coincide, with caption/resize-border style bits and extended edge style bits cleared only while docked.
- Force the initial compact-state IPC to reject in an isolated test renderer. Verify the UI remains unready, then physically resize it to another legal compact size. The retry must preserve the event listener and update the attached layout when the native Docked event arrives.
- Detach and compare the native parent, styles, physical screen rectangle, logical compact size and renderer frame memory with the original values. Repeat on a supported secondary horizontal taskbar.
- Drag close to the taskbar and check automatic attachment after movement settles. An unsupported layout or insufficient free space must show a waiting reason without covering shell controls.
- Put a real fullscreen test window in the foreground on the dock's monitor. Verify native visibility is false, reopen through the normal tray/single-instance path, then close the fullscreen window and verify automatic recovery without foreground focus changing to the player. Also check that another monitor's taskbar observation still has valid monitor bounds.
- Exercise native minimize, explicit close-to-tray, and their ordinary reopen paths. They must remain hidden/minimized through polling until the user asks to reopen.
- Stop only this host's native helper and confirm bounded recovery restores the same player HWND and attachment intent. Do not restart Explorer or another session's helper for this check.
- Double-click the attached player and verify it stays docked and compact. Click its return-to-floating button and verify the original compact frame, parent and styles are restored. Double-click the floating compact player and verify its existing browse transition still works. Quit while docked and verify the native frame is restored before exit; the host records the verified restore in its diagnostic log.
- Attempt to enable docking or report compact mode from a normal, maximized or fullscreen main window. Native validation must reject it without reparenting the main HWND.

Windows native tests are distinct from model/unit tests. A source review of the Windows guards also does not constitute a macOS or Linux runtime test.
