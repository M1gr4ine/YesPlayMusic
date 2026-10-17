import { describe, expect, test } from 'bun:test';
import {
  decodeDockHint,
  decodeDockState,
  decodeMainMessage,
  decodeStripMessage,
  decodeStripPhase,
} from '../src/services/taskbarOverlayChannel';

// The strip renders whatever arrives on a BroadcastChannel any same-origin
// page can post to, so every decoder must fail closed on unexpected shapes.

const snapshot = {
  artwork: 'https://p1.music.126.net/cover.jpg?param=128y128',
  title: 'Song',
  artist: 'Artist',
  lyric: 'Line',
  lyricLoading: false,
  progress: 12.5,
  duration: 180,
  playing: true,
  locale: 'zh-CN',
} as const;

describe('任务栏歌词条协议', () => {
  test('播放快照逐字段校验，并把进度夹在时长以内', () => {
    expect(decodeMainMessage({ type: 'snapshot', snapshot })).toEqual({
      type: 'snapshot',
      snapshot,
    });
    expect(
      decodeMainMessage({
        type: 'snapshot',
        snapshot: { ...snapshot, progress: 999 },
      })
    ).toEqual({ type: 'snapshot', snapshot: { ...snapshot, progress: 180 } });

    expect(
      decodeMainMessage({
        type: 'phase',
        phase: { phase: 'docking', motion: 'instant', durationMs: 0 },
      })
    ).toEqual({
      type: 'phase',
      phase: { phase: 'docking', motion: 'instant', durationMs: 0 },
    });

    const broken: unknown[] = [
      null,
      { type: 'snapshot' },
      { type: 'phase', phase: { phase: 'docked', motion: 'slide' } },
      { type: 'snapshot', snapshot: { ...snapshot, locale: 'tr' } },
      { type: 'snapshot', snapshot: { ...snapshot, progress: -1 } },
      { type: 'snapshot', snapshot: { ...snapshot, duration: NaN } },
      { type: 'snapshot', snapshot: { ...snapshot, playing: 'yes' } },
      { type: 'snapshot', snapshot: { ...snapshot, title: 1 } },
      { type: 'other', snapshot },
    ];
    for (const message of broken) {
      expect(decodeMainMessage(message)).toBeNull();
    }
  });

  test('歌词条只能发出已知动作和非负的跳转秒数', () => {
    expect(decodeStripMessage({ type: 'ready' })).toEqual({ type: 'ready' });
    expect(decodeStripMessage({ type: 'action', action: 'undock' })).toEqual({
      type: 'action',
      action: 'undock',
    });
    expect(decodeStripMessage({ type: 'seek', seconds: 30 })).toEqual({
      type: 'seek',
      seconds: 30,
    });
    for (const message of [
      { type: 'action', action: 'quit' },
      { type: 'seek', seconds: -1 },
      { type: 'seek', seconds: '30' },
      { type: 'seek', seconds: Infinity },
      'ready',
    ]) {
      expect(decodeStripMessage(message)).toBeNull();
    }
  });

  test('Rust 事件的相位、提示和吸附状态按枚举解析', () => {
    expect(
      decodeStripPhase({ phase: 'docking', motion: 'fly', durationMs: 240 })
    ).toEqual({ phase: 'docking', motion: 'fly', durationMs: 240 });
    expect(
      decodeStripPhase({ phase: 'hidden', motion: null, durationMs: 0 })
    ).toEqual({ phase: 'hidden', motion: null, durationMs: 0 });
    expect(
      decodeStripPhase({ phase: 'docked', motion: 'slide', durationMs: 0 })
    ).toBeNull();
    expect(decodeStripPhase({ phase: 'flying', durationMs: 0 })).toBeNull();

    expect(decodeDockHint({ state: 'ready' })).toBe('ready');
    expect(decodeDockHint({ state: 'maybe' })).toBeNull();
    expect(decodeDockState({ docked: false })).toBe(false);
    expect(decodeDockState({ docked: 'true' })).toBeNull();
  });
});
