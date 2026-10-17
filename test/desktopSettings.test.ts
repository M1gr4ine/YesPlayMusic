import { expect, test } from 'bun:test';
import {
  createOrderedDesktopSettings,
  withSystemPreferences,
} from '../src/services/desktopSettings';

test('the OS motion preference rides along with the settings', () => {
  // Rust rejects a settings payload without it, so the key is load-bearing.
  expect(withSystemPreferences({ closeAppOption: 'ask' }, true)).toEqual({
    closeAppOption: 'ask',
    systemReducedMotion: true,
  });
  expect(withSystemPreferences(null, true)).toBeNull();
});

test('Discord presence waits for the first settings configuration', async () => {
  const calls: Array<[string, unknown]> = [];
  const transport = createOrderedDesktopSettings(async (channel, payload) => {
    calls.push([channel, payload]);
  });

  const presence = transport.sendDiscordPresence({ title: 'Track' });
  await Promise.resolve();
  expect(calls).toEqual([]);

  await transport.sync({ enableDiscordRichPresence: true });
  await presence;
  expect(calls).toEqual([
    ['settings', { enableDiscordRichPresence: true }],
    ['discordPresence', { title: 'Track' }],
  ]);
});

test('settings updates are serialized before the following presence', async () => {
  const calls: string[] = [];
  const transport = createOrderedDesktopSettings(async (channel, payload) => {
    const enabled =
      typeof payload === 'object' && payload !== null
        ? Reflect.get(payload, 'enableDiscordRichPresence')
        : undefined;
    calls.push(`${channel}:${String(enabled ?? '')}`);
  });

  const first = transport.sync({ enableDiscordRichPresence: false });
  const second = transport.sync({ enableDiscordRichPresence: true });
  const presence = transport.sendDiscordPresence({ title: 'Track' });
  await Promise.all([first, second, presence]);

  expect(calls).toEqual([
    'settings:false',
    'settings:true',
    'discordPresence:',
  ]);
});
