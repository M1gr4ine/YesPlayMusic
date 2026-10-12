# QQ Music quick login on Windows

In Settings, beside the existing QQ Music cookie field, use the official-login button. The app opens QQ Music's official profile/login page in an isolated temporary window and opens the site's login dialog when its profile page loads. Complete the login shown by QQ Music; a valid QQ Music session is imported into the existing cookie setting for UnblockNeteaseMusic playback.

The login window has its own temporary browser data directory and uses WebView2 InPrivate mode. It can navigate only to HTTPS QQ domains, denies browser permission requests, and has no app IPC capability. Only QQ Music cookie names from the QQ Music domains are collected, including HttpOnly cookies. Imported cookie values are never logged.

The current setting is preserved when login is canceled, cookies are incomplete, or the setting is edited while login is in progress. Login without a refresh token is accepted only after its cookie candidate remains stable. The login window closes after the main renderer acknowledges that it has saved the setting.

The manual cookie field remains available on other platforms. This feature does not replace NetEase login or its QR-code flow.

## Verification

Cookie normalization, domain/name filtering, candidate stability, cancellation and conflicting settings changes are covered by automated tests. Native Windows verification checks the dedicated WebView2 session, official-page navigation restrictions, denied permissions, HttpOnly-cookie capture and acknowledged persistence. Tests use fake cookie values; a real QQ account login requires completing QQ Music's official interactive authentication.

## QQ playback compatibility

The QQ provider sends `guid` as a string and requests the matched media file's `media_mid`, rather than treating the catalog song ID as a file ID. An empty `purl` is rejected before forming an audio URL. It uses the existing imported QQ session and source settings, so authorization does not need to be repeated after installing the updated backend.

Provider tests cover request shape, media identifiers, duration matching, response validation and empty playback results. In the combined Windows build, an already authorized QQ source resolved Jay Chou's Qing Tian and Qi Li Xiang, and both audio range requests returned HTTP 206 with 1024 bytes. Session values and signed playback URLs were neither saved nor logged. Automatic session renewal is not implemented.
