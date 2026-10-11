import { describe, expect, test } from 'bun:test';
import {
  applyCompactExpansionFrameWithRollback,
  type CompactWindowFrame,
} from '../src/services/compactWindow';

describe('Windows compact expansion native rollback', () => {
  const bar: CompactWindowFrame = { x: 100, y: 850, width: 520, height: 100 };
  const browse: CompactWindowFrame = {
    x: 100,
    y: 330,
    width: 920,
    height: 620,
  };

  test('restores the compact frame when a native resize fails after changing size', async () => {
    const applied: CompactWindowFrame[] = [];
    const nativeFailure = new Error('position failed after resize');
    const apply = async (frame: CompactWindowFrame) => {
      applied.push(frame);
      if (frame === browse) throw nativeFailure;
      return true;
    };
    await expect(
      applyCompactExpansionFrameWithRollback(bar, browse, apply)
    ).rejects.toBe(nativeFailure);
    expect(applied).toEqual([browse, bar]);
  });

  test('does not restore geometry after a successful expansion', async () => {
    const applied: CompactWindowFrame[] = [];
    expect(
      await applyCompactExpansionFrameWithRollback(bar, browse, async frame => {
        applied.push(frame);
        return true;
      })
    ).toBe(true);
    expect(applied).toEqual([browse]);
  });
});
