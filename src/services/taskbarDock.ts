import { sendDesktop } from '@/services/desktopTransport';
import {
  decodeDockHint,
  decodeDockState,
  decodeStripMessage,
  decodeStripPhase,
  openStripChannel,
} from '@/services/taskbarOverlayChannel';
import type {
  TaskbarDockHint,
  TaskbarStripAction,
  TaskbarStripPhase,
  TaskbarStripSnapshot,
} from '@/services/taskbarOverlayChannel';

// Player-page side of Windows taskbar docking. Rust reports drag hints,
// whether the strip is active and the strip's phase; the strip itself only
// talks over the channel, so the phase is relayed to it from here.

export type TaskbarPlaybackAction = Exclude<
  TaskbarStripAction,
  'undock' | 'hide'
>;

export interface TaskbarDockHandlers {
  onHint(hint: TaskbarDockHint): void;
  onDocked(docked: boolean): void;
  onAction(action: TaskbarPlaybackAction): void;
  onSeek(seconds: number): void;
}

export interface TaskbarDockSession {
  /** Mirrors playback into the strip while it is active. */
  publish(snapshot: TaskbarStripSnapshot): void;
  dispose(): void;
}

async function listenDesktop(
  event: string,
  handler: (payload: unknown) => void
): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  return listen<unknown>(`desktop://${event}`, ({ payload }) =>
    handler(payload)
  );
}

export function connectTaskbarDock(
  handlers: TaskbarDockHandlers
): TaskbarDockSession {
  let disposed = false;
  let docked = false;
  let latest: TaskbarStripSnapshot | null = null;
  let phase: TaskbarStripPhase | null = null;
  let publishPending = false;
  const channel = openStripChannel();

  const send = () => {
    publishPending = false;
    if (!latest || !docked || disposed) return;
    channel?.postMessage({ type: 'snapshot', snapshot: latest });
  };
  // A microtask coalesces one render's worth of changes and, unlike a
  // timer, is not throttled while this page sits in a hidden window.
  const schedule = () => {
    if (publishPending) return;
    publishPending = true;
    queueMicrotask(send);
  };
  const relayPhase = () => {
    if (phase && !disposed) channel?.postMessage({ type: 'phase', phase });
  };

  if (channel) {
    channel.onmessage = event => {
      const message = decodeStripMessage(event.data);
      if (!message || disposed) return;
      switch (message.type) {
        case 'ready':
          // The strip just loaded: hand it the current mirror and the last
          // phase, and tell Rust so a pending dock can go ahead.
          send();
          relayPhase();
          void sendDesktop('taskbarDocking', { action: 'stripReady' });
          return;
        case 'seek':
          handlers.onSeek(message.seconds);
          return;
        case 'action':
          if (message.action === 'undock' || message.action === 'hide') {
            void sendDesktop('taskbarDocking', { action: message.action });
          } else {
            handlers.onAction(message.action);
          }
      }
    };
  }

  const subscriptions = Promise.allSettled([
    listenDesktop('taskbarDockHint', payload => {
      const hint = decodeDockHint(payload);
      if (hint) handlers.onHint(hint);
    }),
    listenDesktop('taskbarDockState', payload => {
      const next = decodeDockState(payload);
      if (next === null) return;
      docked = next;
      handlers.onDocked(next);
      if (next) send();
    }),
    listenDesktop('taskbarStripPhase', payload => {
      const next = decodeStripPhase(payload);
      if (!next) return;
      phase = next;
      relayPhase();
    }),
  ]).then(results =>
    results.flatMap(result =>
      result.status === 'fulfilled' ? [result.value] : []
    )
  );

  return {
    publish(snapshot) {
      latest = snapshot;
      if (docked) schedule();
    },
    dispose() {
      disposed = true;
      channel?.close();
      void subscriptions.then(stops => stops.forEach(stop => stop()));
    },
  };
}
