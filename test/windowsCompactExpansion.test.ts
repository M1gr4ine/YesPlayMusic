import { expect, test } from 'bun:test';
import { buildWindowsCompactExpansionFrame } from '../src/services/compactWindow';

const area = { x: 0, y: 0, width: 1920, height: 1040 };
const target = { x: null, y: null, width: 920, height: 620 };

test('a compact window in the upper half expands downward from its top edge', () => {
  expect(
    buildWindowsCompactExpansionFrame(
      { x: 100, y: 120, width: 520, height: 80 },
      target,
      area,
      1
    )
  ).toEqual({ x: 100, y: 120, width: 920, height: 620 });
});

test('a compact window in the lower half expands upward from its bottom edge', () => {
  expect(
    buildWindowsCompactExpansionFrame(
      { x: 100, y: 900, width: 520, height: 80 },
      target,
      area,
      1
    )
  ).toEqual({ x: 100, y: 360, width: 920, height: 620 });
});

test('expansion respects negative monitor origins, taskbars and physical DPI coordinates', () => {
  expect(
    buildWindowsCompactExpansionFrame(
      { x: -500, y: 1300, width: 520, height: 80 },
      target,
      { x: -2560, y: 40, width: 2560, height: 1400 },
      1.5
    )
  ).toEqual({ x: -1380, y: 490, width: 920, height: 620 });
});

test('a remembered browse size larger than the display fits the current work area', () => {
  expect(
    buildWindowsCompactExpansionFrame(
      { x: 800, y: 650, width: 520, height: 80 },
      { x: 0, y: 0, width: 1600, height: 1200 },
      { x: 0, y: 0, width: 1280, height: 720 },
      1
    )
  ).toEqual({ x: 0, y: 0, width: 1280, height: 720 });
});

test('missing monitor position or invalid monitor scale preserves the original transition', () => {
  expect(
    buildWindowsCompactExpansionFrame(
      { x: null, y: null, width: 520, height: 80 },
      target,
      area,
      1
    )
  ).toEqual(target);
  expect(
    buildWindowsCompactExpansionFrame(
      { x: 100, y: 900, width: 520, height: 80 },
      target,
      area,
      0
    )
  ).toEqual({ x: 100, y: 900, width: 920, height: 620 });
});
