import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import {
  MINI_WINDOW_INTERACTIVE_SELECTOR,
  beginMiniWindowDragGesture,
  hasCrossedMiniWindowDragThreshold,
  isBarWindowSize,
  isMiniWindowSize,
  shouldStartMiniWindowDrag,
  shouldToggleMiniWindow,
} from '../src/utils/miniWindow';

const lyricsView = readFileSync(
  new URL('../src/views/lyrics.vue', import.meta.url),
  'utf8'
);

function targetMatching(selector: string | null = null): EventTarget {
  return Object.assign(Object.create(null) as EventTarget, {
    closest(query: string) {
      expect(query).toBe(MINI_WINDOW_INTERACTIVE_SELECTOR);
      return selector ? { selector } : null;
    },
  });
}

describe('迷你窗口拖拽边界', () => {
  test('封面和空白区允许拖动窗口', () => {
    expect(
      shouldStartMiniWindowDrag({
        button: 0,
        detail: 1,
        target: targetMatching(),
      })
    ).toBe(true);
  });

  test('文字、播放控件和进度条保留自己的交互', () => {
    for (const selector of [
      '.mini-copyable',
      '.mini-controls',
      '.mini-progress-track',
    ]) {
      expect(
        shouldStartMiniWindowDrag({
          button: 0,
          detail: 1,
          target: targetMatching(selector),
        })
      ).toBe(false);
    }
  });

  test('只有文字本身可选择，容器空白仍用于拖窗和双击', () => {
    expect(lyricsView).toContain(
      '<span class="mini-copyable">{{ currentTrack.name }}</span>'
    );
    expect(lyricsView).toContain(
      '<span class="mini-copyable">{{ displayLyric }}</span>'
    );
    expect(lyricsView).not.toContain('mini-info mini-copyable');
    expect(lyricsView).not.toContain('mini-lyric mini-copyable');
  });

  test('从空白处起手的拖窗当场掐掉选中起点，压在文字上则不拦', () => {
    // Prevent WebKit from anchoring a selection before window dragging.
    let prevented = 0;
    const blank = {
      button: 0,
      detail: 1,
      target: targetMatching(),
      preventDefault: () => {
        prevented += 1;
      },
    };
    expect(beginMiniWindowDragGesture(blank)).toBe(true);
    expect(prevented).toBe(1);

    const onText = {
      button: 0,
      detail: 1,
      target: targetMatching('.mini-copyable'),
      preventDefault: () => {
        prevented += 1;
      },
    };
    expect(beginMiniWindowDragGesture(onText)).toBe(false);
    expect(prevented).toBe(1);
  });

  test('按空白处会清掉已有的选中，按在文字上不清', () => {
    // preventDefault also blocks native selection clearing.
    let cleared = 0;
    const selection = Object.assign(Object.create(null) as Selection, {
      removeAllRanges: () => {
        cleared += 1;
      },
    });

    beginMiniWindowDragGesture(
      { button: 0, detail: 1, target: targetMatching(), preventDefault() {} },
      selection
    );
    expect(cleared).toBe(1);

    beginMiniWindowDragGesture(
      {
        button: 0,
        detail: 1,
        target: targetMatching('.mini-copyable'),
        preventDefault() {},
      },
      selection
    );
    expect(cleared).toBe(1);
  });

  test('右键和双击不会误启动单击拖动', () => {
    expect(
      shouldStartMiniWindowDrag({
        button: 2,
        detail: 1,
        target: targetMatching(),
      })
    ).toBe(false);
    expect(
      shouldStartMiniWindowDrag({
        button: 0,
        detail: 2,
        target: targetMatching(),
      })
    ).toBe(false);
  });

  test('移动超过阈值才交给原生窗口拖拽', () => {
    const start = { clientX: 100, clientY: 100 };
    expect(
      hasCrossedMiniWindowDragThreshold(start, { clientX: 102, clientY: 102 })
    ).toBe(false);
    expect(
      hasCrossedMiniWindowDragThreshold(start, { clientX: 104, clientY: 100 })
    ).toBe(true);
  });

  test('双击非文本区域进入中窗，文本和控件不触发', () => {
    expect(
      shouldToggleMiniWindow({ button: 0, target: targetMatching() })
    ).toBe(true);
    expect(
      shouldToggleMiniWindow({
        button: 0,
        target: targetMatching('.mini-copyable'),
      })
    ).toBe(false);
  });

  test('迷你模式沿用宽度或高度任一不足的现有规则', () => {
    expect(isMiniWindowSize({ width: 1440, height: 103 })).toBe(true);
    expect(isMiniWindowSize({ width: 560, height: 800 })).toBe(true);
    expect(isMiniWindowSize({ width: 920, height: 620 })).toBe(false);
  });

  test('播放条只属于矮窗口，窄而高保持完整播放器视图', () => {
    expect(isBarWindowSize({ width: 1440, height: 103 })).toBe(true);
    expect(isBarWindowSize({ width: 560, height: 800 })).toBe(false);
    expect(isBarWindowSize({ width: 300, height: 500 })).toBe(false);
    // 窄高窗口仍会路由到歌词页，只是不进播放条布局。
    expect(isMiniWindowSize({ width: 560, height: 800 })).toBe(true);
  });
});
