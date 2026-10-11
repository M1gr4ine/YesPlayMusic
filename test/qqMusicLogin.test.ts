import { describe, expect, test } from 'bun:test';
import {
  normalizeQQMusicCookies,
  normalizeQQMusicCookieValue,
  persistQQMusicLoginCandidate,
  startDesktopQQMusicLogin,
} from '../src/services/qqMusicLogin';

function qqCookie(name: string, value: string, domain = '.qq.com') {
  return { name, value, domain };
}

describe('QQ Music login cookie normalization', () => {
  test('keeps only the QQ cookie names on the supported QQ domains', () => {
    const result = normalizeQQMusicCookies([
      qqCookie('uin', '123456789'),
      qqCookie('qm_keyst', 'fake-key'),
      qqCookie('psrf_qqrefresh_token', 'fake-refresh-token', '.y.qq.com'),
      qqCookie('sessionid', 'fake-session'),
      qqCookie('uin', '999999999', '.login.qq.com'),
      qqCookie('qqmusic_key', 'ignored', '.evil.qq.com'),
    ]);

    expect(result).toEqual({
      value:
        'uin=123456789; qqmusic_key=fake-key; qm_keyst=fake-key; psrf_qqrefresh_token=fake-refresh-token',
      valid: true,
      canRefresh: true,
      empty: false,
    });
  });

  test('accepts either key alias and writes both names for UNM', () => {
    expect(
      normalizeQQMusicCookies([
        qqCookie('uin', '123456789'),
        qqCookie('qqmusic_key', 'fake-key'),
      ])
    ).toMatchObject({
      value: 'uin=123456789; qqmusic_key=fake-key; qm_keyst=fake-key',
      valid: true,
      canRefresh: false,
    });
  });

  test('rejects malformed UINs and values containing whitespace or controls', () => {
    expect(
      normalizeQQMusicCookieValue('uin=o123; qm_keyst=fake-key').valid
    ).toBe(false);
    expect(
      normalizeQQMusicCookieValue('uin=123; qm_keyst=fake key').valid
    ).toBe(false);
    expect(
      normalizeQQMusicCookieValue('uin=123; qm_keyst=fake-key\u0001').valid
    ).toBe(false);
  });

  test('returns an empty normalized result for no cookies', () => {
    expect(normalizeQQMusicCookies([])).toEqual({
      value: '',
      valid: false,
      canRefresh: false,
      empty: true,
    });
  });
});

describe('QQ Music login commit boundary', () => {
  test('preserves a manual edit made while the login window was open', () => {
    let current = 'uin=111; qm_keyst=old-key';
    let persisted = current;
    let updates = 0;

    const result = persistQQMusicLoginCandidate(
      {
        attemptId: 'attempt-1',
        value:
          'uin=222; qqmusic_key=fake-key; qm_keyst=fake-key; psrf_qqrefresh_token=fake-refresh-token',
        canRefresh: true,
      },
      'uin=111; qm_keyst=old-key',
      {
        getCurrent: () => current,
        getPersisted: () => persisted,
        update: value => {
          updates += 1;
          current = value;
          persisted = value;
        },
      }
    );

    expect(result).toBe('committed');
    expect(current).toBe(
      'uin=222; qqmusic_key=fake-key; qm_keyst=fake-key; psrf_qqrefresh_token=fake-refresh-token'
    );
    expect(persisted).toBe(current);
    expect(updates).toBe(1);
  });

  test('does not overwrite the current cookie if it changed after the attempt began', () => {
    let current = 'uin=333; qm_keyst=manual-key';
    let updates = 0;

    const result = persistQQMusicLoginCandidate(
      {
        attemptId: 'attempt-2',
        value:
          'uin=222; qqmusic_key=fake-key; qm_keyst=fake-key; psrf_qqrefresh_token=fake-refresh-token',
        canRefresh: true,
      },
      'uin=111; qm_keyst=old-key',
      {
        getCurrent: () => current,
        getPersisted: () => current,
        update: () => {
          updates += 1;
        },
      }
    );

    expect(result).toBe('conflict');
    expect(current).toBe('uin=333; qm_keyst=manual-key');
    expect(updates).toBe(0);
  });

  test('requires local settings persistence before acknowledging a candidate', () => {
    let current = 'uin=111; qm_keyst=old-key';
    let persisted = current;

    const result = persistQQMusicLoginCandidate(
      {
        attemptId: 'attempt-3',
        value:
          'uin=222; qqmusic_key=fake-key; qm_keyst=fake-key; psrf_qqrefresh_token=fake-refresh-token',
        canRefresh: true,
      },
      current,
      {
        getCurrent: () => current,
        getPersisted: () => persisted,
        update: value => {
          current = value;
        },
      }
    );

    expect(result).toBe('persistence-failed');
    expect(current).toBe('uin=111; qm_keyst=old-key');
    expect(persisted).toBe('uin=111; qm_keyst=old-key');
  });
});

describe('QQ Music login native handshake', () => {
  test('acknowledges the saved candidate before reporting completion', async () => {
    let receiveCandidate: ((payload: unknown) => void) | undefined;
    const steps: string[] = [];
    let complete!: () => void;
    const completed = new Promise<void>(resolve => {
      complete = resolve;
    });

    const cleanup = await startDesktopQQMusicLogin(
      {
        onCandidate: () => {
          steps.push('persist');
          return 'committed';
        },
        onComplete: () => {
          steps.push('complete');
          complete();
        },
      },
      {
        listenForCandidate: async handler => {
          receiveCandidate = handler;
          return () => {};
        },
        open: async () => ({ attemptId: 'attempt-1', reused: false }),
        finish: async request => {
          steps.push(request.committed ? 'ack-committed' : 'ack-rejected');
        },
      }
    );

    receiveCandidate?.({
      attemptId: 'attempt-1',
      value:
        'uin=222; qqmusic_key=fake-key; qm_keyst=fake-key; psrf_qqrefresh_token=fake-refresh-token',
      canRefresh: true,
    });
    await completed;
    expect(steps).toEqual(['persist', 'ack-committed', 'complete']);
    cleanup();
  });

  test('uses the candidate event close sentinel and ignores other attempts', async () => {
    let receiveCandidate: ((payload: unknown) => void) | undefined;
    let closed = 0;
    let unlistened = 0;
    let candidates = 0;
    const cleanup = await startDesktopQQMusicLogin(
      {
        onCandidate: () => {
          candidates += 1;
          return 'committed';
        },
        onComplete: () => {},
        onClosed: () => {
          closed += 1;
        },
      },
      {
        listenForCandidate: async handler => {
          receiveCandidate = handler;
          return () => {
            unlistened += 1;
          };
        },
        open: async () => ({ attemptId: 'attempt-2', reused: false }),
        finish: async () => {},
      }
    );

    receiveCandidate?.({
      attemptId: 'different-attempt',
      value: '',
      canRefresh: false,
    });
    receiveCandidate?.({
      attemptId: 'attempt-2',
      value: '',
      canRefresh: false,
    });
    expect(closed).toBe(1);
    expect(unlistened).toBe(1);
    expect(candidates).toBe(0);
    cleanup();
  });

  test('rolls back a saved candidate if the native success acknowledgement fails', async () => {
    let receiveCandidate: ((payload: unknown) => void) | undefined;
    let rolledBack = false;
    let failed!: () => void;
    const failureHandled = new Promise<void>(resolve => {
      failed = resolve;
    });

    const cleanup = await startDesktopQQMusicLogin(
      {
        onCandidate: () => 'committed',
        onComplete: () => {},
        onNativeRejected: () => {
          rolledBack = true;
        },
        onError: () => failed(),
      },
      {
        listenForCandidate: async handler => {
          receiveCandidate = handler;
          return () => {};
        },
        open: async () => ({ attemptId: 'attempt-3', reused: false }),
        finish: async request => {
          if (request.committed) throw new Error('fake native failure');
        },
      }
    );

    receiveCandidate?.({
      attemptId: 'attempt-3',
      value:
        'uin=222; qqmusic_key=fake-key; qm_keyst=fake-key; psrf_qqrefresh_token=fake-refresh-token',
      canRefresh: true,
    });
    await failureHandled;
    expect(rolledBack).toBe(true);
    cleanup();
  });
});
