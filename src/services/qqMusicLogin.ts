// Opens QQ Music's official login window (Windows desktop only). Resolves with
// the cookie for the UNM QQ engine, or null when the user closes the window or
// one is already open (Rust focuses it; the first call delivers the cookie).
export async function openQQMusicLogin(): Promise<string | null> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<string | null>('qq_music_login');
}
