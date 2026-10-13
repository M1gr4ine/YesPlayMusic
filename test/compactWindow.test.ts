import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import {
  buildCompactWindowTransitionFrame,
  COMPACT_RESIZE_SETTLE_MS,
  fitCompactExpansionFrame,
  isCompactWindowPhysicalSize,
  loadCompactWindowMemory,
  rememberCompactWindowFrame,
} from '../src/services/compactWindow';
import { createMemoryStorage } from './helpers/memoryStorage';

const app = readFileSync(new URL('../src/App.vue', import.meta.url), 'utf8');
const lyrics = readFileSync(
  new URL('../src/views/lyrics.vue', import.meta.url),
  'utf8'
);
const navbar = readFileSync(
  new URL('../src/components/Navbar.vue', import.meta.url),
  'utf8'
);
const compactWindow = readFileSync(
  new URL('../src/services/compactWindow.ts', import.meta.url),
  'utf8'
);
const tauriMain = readFileSync(
  new URL('../src-tauri/src/main.rs', import.meta.url),
  'utf8'
);
const windowPreferences = readFileSync(
  new URL('../src-tauri/src/window_preferences.rs', import.meta.url),
  'utf8'
);

test('小窗双击进入播放队列，中窗提供明确的返回入口', () => {
  expect(lyrics).toContain('@dblclick="handleMiniDoubleClick"');
  expect(lyrics).toContain("this.$emit('expand-compact-window')");
  expect(app).toContain("this.$router.push({ name: 'next' })");
  expect(navbar).toContain(
    ":title=\"$t('nav.restoreCompactPlayerTitle', { shortcut: 'Esc' })\""
  );
  expect(navbar).toContain("$emit('restore-compact-window')");
});

test('Retina 物理像素先换算成逻辑像素再判断小窗', () => {
  expect(isCompactWindowPhysicalSize({ width: 988, height: 508 }, 2)).toBe(
    true
  );
  expect(isCompactWindowPhysicalSize({ width: 1840, height: 1240 }, 2)).toBe(
    false
  );
});

test('Tauri 的恢复路径拒绝恢复到屏幕外', () => {
  expect(compactWindow).toContain("invoke('restore_compact_window'");
  expect(tauriMain).toContain('ensure_main_window_reachable(&window)?;');
});

test('Bar 和浏览尺寸分别记忆，更新一档不会覆盖另一档', () => {
  const storage = createMemoryStorage();

  rememberCompactWindowFrame({ x: 20, y: 30, width: 560, height: 72 }, storage);
  rememberCompactWindowFrame(
    { x: 100, y: 80, width: 1080, height: 700 },
    storage
  );
  rememberCompactWindowFrame({ x: 40, y: 50, width: 500, height: 64 }, storage);

  expect(loadCompactWindowMemory(storage)).toEqual({
    bar: { x: 40, y: 50, width: 500, height: 64 },
    browse: { x: 100, y: 80, width: 1080, height: 700 },
    lastMode: 'bar',
  });
  expect(COMPACT_RESIZE_SETTLE_MS).toBeGreaterThanOrEqual(200);
});

test('窄而高的窗口按浏览档记忆，不污染播放条档', () => {
  const storage = createMemoryStorage();

  rememberCompactWindowFrame({ x: 0, y: 0, width: 600, height: 890 }, storage);

  expect(loadCompactWindowMemory(storage)).toEqual({
    bar: null,
    browse: { x: 0, y: 0, width: 600, height: 890 },
    lastMode: 'browse',
  });
});

test('双屏切换档位时沿用当前屏位置，只恢复目标档位尺寸', () => {
  const currentOnRetina = { x: 5480, y: 220, width: 920, height: 620 };
  const rememberedBarOnExternal = { x: 180, y: 90, width: 520, height: 72 };

  expect(
    buildCompactWindowTransitionFrame(currentOnRetina, rememberedBarOnExternal)
  ).toEqual({ x: 5480, y: 220, width: 520, height: 72 });
});

test('中窗提供明确返回按钮和 Escape 快捷键', () => {
  expect(navbar).toContain("$t('nav.restoreCompactPlayer')");
  expect(navbar).toContain('<kbd>Esc</kbd>');
  expect(app).toContain("e.code === 'Escape'");
  expect(app).toContain('this.restoreCompactWindow()');
});

test('重启时恢复最后使用的逻辑尺寸，不采用 Tauri 插件保存的物理像素', () => {
  const storage = createMemoryStorage();

  rememberCompactWindowFrame(
    { x: 20, y: 30, width: 346, height: 177 },
    storage
  );
  rememberCompactWindowFrame(
    { x: 100, y: 80, width: 1060, height: 720 },
    storage
  );

  expect(loadCompactWindowMemory(storage)).toEqual({
    bar: { x: 20, y: 30, width: 346, height: 177 },
    browse: { x: 100, y: 80, width: 1060, height: 720 },
    lastMode: 'browse',
  });
  expect(compactWindow).toContain('restoreRememberedCompactWindowFrame');
  expect(compactWindow).toContain(
    'buildCompactWindowTransitionFrame(current, frame)'
  );
  expect(app).toContain('compactWindowMemoryReady');
  expect(tauriMain).toContain('.skip_initial_state("main")');
});

test('native 保存位置和一次性旧尺寸，compact 仍管理后续逻辑宽高', () => {
  expect(tauriMain).toContain('WindowEvent::Moved(position)');
  expect(tauriMain).toContain('schedule_window_position_save(');
  expect(tauriMain).toContain('persist_current_window_position(app)');
  expect(windowPreferences).toContain('tempfile::NamedTempFile::new_in');
  expect(windowPreferences).toContain('pub position: Option<WindowPosition>');
  expect(windowPreferences).toContain('pub size: Option<WindowSize>');
  expect(tauriMain).toContain('preferences.size.unwrap_or(WindowSize');
  expect(tauriMain).not.toContain('schedule_window_size_save');
});

test('启动等待 renderer 恢复尺寸，超时兜底不影响 hidden smoke', () => {
  expect(tauriMain).toContain('.visible(false)');
  expect(tauriMain).toContain('fn renderer_ready(');
  expect(tauriMain).toContain('schedule_startup_show_fallback(app.handle())');
  expect(app).toContain('await signalInitialWindowReady()');

  const branchStart = tauriMain.indexOf('} else if webview_smoke_test {');
  const smokeBranch = tauriMain.slice(
    branchStart,
    tauriMain.indexOf('} else {', branchStart)
  );
  expect(smokeBranch).not.toContain('schedule_startup_show_fallback');
});

test('恢复主窗口时先解除最小化再显示和聚焦', () => {
  const showMainWindow = tauriMain.slice(
    tauriMain.indexOf('fn show_main_window('),
    tauriMain.indexOf('fn show_pending_startup_window(')
  );
  expect(showMainWindow.indexOf('window.unminimize()')).toBeGreaterThan(-1);
  expect(showMainWindow.indexOf('window.unminimize()')).toBeLessThan(
    showMainWindow.indexOf('window.show()')
  );
});

test('Windows 和 Linux 从 mini bar 展开时先退出最大化且不记忆全屏尺寸', () => {
  const restoreCommand = tauriMain.slice(
    tauriMain.indexOf('fn restore_compact_window('),
    tauriMain.indexOf('fn create_tray(')
  );
  expect(restoreCommand.indexOf('window.unmaximize()')).toBeGreaterThan(-1);
  expect(restoreCommand.indexOf('window.unmaximize()')).toBeLessThan(
    restoreCommand.indexOf('.set_size(')
  );
  expect(compactWindow).toContain('window.isMaximized()');
  expect(compactWindow).toContain('window.isFullscreen()');
  expect(compactWindow).toContain(
    'if (!frame || !snapshot.normal) return null'
  );
  expect(compactWindow).toContain('await window.setFullscreen(false)');
});

describe('Windows compact expansion frame', () => {
  const browse = { x: null, y: null, width: 920, height: 620 };
  const area = (x: number, y: number, width: number, height: number) => ({
    position: { x, y },
    size: { width, height },
  });
  const bar = (x: number, y: number) => ({ x, y, width: 520, height: 80 });

  test('grows down from a bar in the upper half', () => {
    expect(
      fitCompactExpansionFrame(bar(100, 120), browse, area(0, 0, 1920, 1040), 1)
    ).toEqual({ x: 100, y: 120, width: 920, height: 620 });
  });

  test('grows up from a bar in the lower half, keeping its bottom edge', () => {
    expect(
      fitCompactExpansionFrame(bar(100, 900), browse, area(0, 0, 1920, 1040), 1)
    ).toEqual({ x: 100, y: 360, width: 920, height: 620 });
  });

  test('handles negative monitor origins and physical DPI coordinates', () => {
    expect(
      fitCompactExpansionFrame(
        bar(-500, 1300),
        browse,
        area(-2560, 40, 2560, 1400),
        1.5
      )
    ).toEqual({ x: -1380, y: 490, width: 920, height: 620 });
  });

  test('shrinks an oversized browse frame into the work area', () => {
    expect(
      fitCompactExpansionFrame(
        bar(800, 650),
        { x: null, y: null, width: 1600, height: 1200 },
        area(0, 0, 1280, 720),
        1
      )
    ).toEqual({ x: 0, y: 0, width: 1280, height: 720 });
  });
});
