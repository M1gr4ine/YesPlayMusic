<template>
  <div
    class="taskbar-strip"
    :data-phase="phase.phase"
    :data-motion="phase.motion ?? 'fade'"
    :style="{ '--phase-duration': `${phase.durationMs}ms` }"
  >
    <div class="flight-glass" aria-hidden="true"></div>
    <div v-if="snapshot" class="strip" :style="{ '--progress': progress }">
      <div class="artwork">
        <img
          v-if="snapshot.artwork"
          :src="snapshot.artwork"
          alt=""
          decoding="async"
          draggable="false"
        />
      </div>
      <div class="text">
        <div class="track">
          <span class="title" :title="snapshot.title">{{
            snapshot.title
          }}</span>
          <span v-if="snapshot.artist" class="artist" :title="snapshot.artist">
            {{ snapshot.artist }}
          </span>
        </div>
        <div class="lyric" :title="snapshot.lyric">
          <template v-if="snapshot.lyricLoading">
            <span class="loading" aria-hidden="true"></span>
            <span class="lyric-muted">{{ $t('taskbar.loadingLyrics') }}</span>
          </template>
          <span v-else class="lyric-line">{{ snapshot.lyric }}</span>
        </div>
      </div>
      <div class="transport">
        <button
          type="button"
          :title="$t('player.previous')"
          :aria-label="$t('player.previous')"
          @click="send('previous')"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M5 5h2v14H5zM18.5 5.5v13L8.5 12z" />
          </svg>
        </button>
        <button
          type="button"
          class="play"
          :title="$t(snapshot.playing ? 'player.pause' : 'player.play')"
          :aria-label="$t(snapshot.playing ? 'player.pause' : 'player.play')"
          @click="send('playPause')"
        >
          <svg v-if="snapshot.playing" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M7 5h4v14H7zM13 5h4v14h-4z" />
          </svg>
          <svg v-else viewBox="0 0 24 24" aria-hidden="true">
            <path d="m8 5.5 10 6.5-10 6.5z" />
          </svg>
        </button>
        <button
          type="button"
          :title="$t('player.next')"
          :aria-label="$t('player.next')"
          @click="send('next')"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M17 5h2v14h-2zM5.5 5.5v13l10-6.5z" />
          </svg>
        </button>
      </div>
      <div class="window-actions">
        <button
          type="button"
          :title="$t('taskbar.undock')"
          :aria-label="$t('taskbar.undock')"
          @click="send('undock')"
        >
          <svg class="stroke" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M4 17.5h16M12 14V4m-4 4 4-4 4 4" />
          </svg>
        </button>
        <button
          type="button"
          :title="$t('taskbar.hide')"
          :aria-label="$t('taskbar.hide')"
          @click="send('hide')"
        >
          <svg class="stroke" viewBox="0 0 24 24" aria-hidden="true">
            <path d="m6.5 6.5 11 11M17.5 6.5l-11 11" />
          </svg>
        </button>
      </div>
      <input
        class="seek"
        type="range"
        min="0"
        step="0.1"
        :max="snapshot.duration"
        :value="shownProgress"
        :disabled="snapshot.duration <= 0"
        :aria-valuetext="`${Math.round(shownProgress)}s`"
        @input="previewSeek"
        @change="commitSeek"
        @pointerup="commitSeek"
        @pointercancel="commitSeek"
        @blur="commitSeek"
      />
    </div>
  </div>
</template>

<script lang="ts">
import { defineComponent, markRaw } from 'vue';
import {
  decodeMainMessage,
  openStripChannel,
} from '@/services/taskbarOverlayChannel';
import type {
  TaskbarStripAction,
  TaskbarStripPhase,
  TaskbarStripSnapshot,
} from '@/services/taskbarOverlayChannel';

const HIDDEN: TaskbarStripPhase = {
  phase: 'hidden',
  motion: null,
  durationMs: 0,
};

// The strip has no Tauri capability and never calls into Rust: snapshots
// and its phase arrive from the player page over the channel, and button
// presses go back the same way.
export default defineComponent({
  name: 'TaskbarStrip',
  data() {
    return {
      snapshot: null as TaskbarStripSnapshot | null,
      phase: HIDDEN,
      channel: null as BroadcastChannel | null,
      // While the thumb is held, snapshots must not yank it back.
      seekPreview: null as number | null,
    };
  },
  computed: {
    shownProgress(): number {
      return this.seekPreview ?? this.snapshot?.progress ?? 0;
    },
    progress(): number {
      if (!this.snapshot || this.snapshot.duration <= 0) return 0;
      return Math.min(1, this.shownProgress / this.snapshot.duration);
    },
  },
  watch: {
    'snapshot.locale'(locale: TaskbarStripSnapshot['locale'] | undefined) {
      if (locale) this.$i18n.locale = locale;
    },
  },
  created() {
    const channel = openStripChannel();
    if (!channel) return;
    channel.onmessage = event => {
      const message = decodeMainMessage(event.data);
      if (!message) return;
      if (message.type === 'snapshot') {
        this.snapshot = message.snapshot;
      } else {
        this.phase = message.phase;
      }
    };
    this.channel = markRaw(channel);
    // The listener is attached, so the replay this triggers cannot be missed.
    channel.postMessage({ type: 'ready' });
  },
  beforeUnmount() {
    this.channel?.close();
  },
  methods: {
    send(action: TaskbarStripAction) {
      this.channel?.postMessage({ type: 'action', action });
    },
    inputSeconds(event: Event): number | null {
      if (!(event.target instanceof HTMLInputElement)) return null;
      const seconds = Number(event.target.value);
      return Number.isFinite(seconds) ? seconds : null;
    },
    previewSeek(event: Event) {
      this.seekPreview = this.inputSeconds(event);
    },
    // Commits the previewed value itself, so a `change` that Chromium
    // delivers after the pointer is released cannot read a stale thumb.
    commitSeek() {
      const seconds = this.seekPreview;
      this.seekPreview = null;
      if (seconds !== null) {
        this.channel?.postMessage({ type: 'seek', seconds });
      }
    },
  },
});
</script>

<style scoped>
.taskbar-strip {
  position: relative;
  width: 100%;
  height: 100%;
  overflow: hidden;
  color: #fff;
  color-scheme: dark;
  font: 12px/1.3 -apple-system, 'Segoe UI Variable', 'Segoe UI',
    'Microsoft YaHei UI', sans-serif;
  -webkit-font-smoothing: antialiased;
  user-select: none;
  -webkit-app-region: no-drag;
}

/* Hidden strips render nothing, so a stale frame can never be revealed. */
.taskbar-strip[data-phase='hidden'] {
  visibility: hidden;
}

/* While flying the strip keeps the bar's glass; landing fades it away. */
.flight-glass {
  position: absolute;
  inset: 0;
  border: 1px solid rgba(255, 255, 255, 0.16);
  border-radius: 10px;
  background: linear-gradient(
    135deg,
    rgba(37, 37, 37, 0.96),
    rgba(22, 22, 22, 0.98)
  );
  opacity: 0;
  pointer-events: none;
}

[data-phase='docking'][data-motion='fly'] .flight-glass {
  animation: glass-out var(--phase-duration) cubic-bezier(0.77, 0, 0.175, 1)
    both;
}

[data-phase='undocking'][data-motion='fly'] .flight-glass {
  animation: glass-in var(--phase-duration) cubic-bezier(0.77, 0, 0.175, 1) both;
}

[data-phase='docking'][data-motion='fade'] .strip {
  animation: fade-in var(--phase-duration) ease-out both;
}

[data-phase='undocking'][data-motion='fade'] .strip {
  animation: fade-out var(--phase-duration) ease-in both;
}

[data-phase='docking'] .strip,
[data-phase='undocking'] .strip {
  pointer-events: none;
}

/* Reduced motion: the window is placed, nothing moves or fades. */
[data-motion='instant'] .flight-glass,
[data-motion='instant'] .strip {
  animation: none;
}

.strip {
  position: relative;
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto auto;
  align-items: center;
  column-gap: 8px;
  width: 100%;
  height: 100%;
  padding: 0 4px 0 0;
  box-sizing: border-box;
}

.artwork {
  width: 100%;
  height: 100%;
  aspect-ratio: 1;
  overflow: hidden;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.08);
}

.artwork img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.text {
  display: flex;
  flex-direction: column;
  justify-content: center;
  min-width: 0;
  gap: 1px;
}

.track {
  display: flex;
  gap: 5px;
  min-width: 0;
  font-size: 10px;
  color: rgba(255, 255, 255, 0.58);
}

.title,
.artist,
.lyric-line,
.lyric-muted {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.artist::before {
  content: '· ';
}

.lyric {
  display: flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  font-size: 13px;
  font-weight: 500;
  letter-spacing: 0.01em;
}

.lyric-muted {
  color: rgba(255, 255, 255, 0.5);
}

.loading {
  flex: none;
  width: 11px;
  height: 11px;
  border: 1.5px solid rgba(255, 255, 255, 0.2);
  border-top-color: rgba(255, 255, 255, 0.75);
  border-radius: 50%;
  animation: spin 900ms linear infinite;
}

button {
  display: grid;
  place-items: center;
  width: 24px;
  height: 28px;
  padding: 0;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: rgba(255, 255, 255, 0.7);
  cursor: pointer;
}

button:hover {
  background: rgba(255, 255, 255, 0.12);
  color: #fff;
}

button:active {
  transform: scale(0.92);
}

button svg {
  width: 14px;
  height: 14px;
  fill: currentColor;
}

button svg.stroke {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.6;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.transport,
.window-actions {
  display: flex;
  align-items: center;
  gap: 1px;
}

.window-actions {
  padding-left: 2px;
  border-left: 1px solid rgba(255, 255, 255, 0.12);
}

/* The last pixel row: a hairline progress bar with an invisible range on top. */
.strip::after {
  position: absolute;
  right: 4px;
  bottom: 0;
  left: 0;
  height: 2px;
  background: linear-gradient(
    to right,
    rgba(255, 255, 255, 0.85) calc(var(--progress) * 100%),
    rgba(255, 255, 255, 0.14) calc(var(--progress) * 100%)
  );
  content: '';
  pointer-events: none;
}

.seek {
  position: absolute;
  right: 4px;
  bottom: 0;
  left: 0;
  width: calc(100% - 4px);
  height: 6px;
  margin: 0;
  padding: 0;
  opacity: 0;
  cursor: pointer;
  appearance: none;
}

.seek:disabled {
  cursor: default;
}

@keyframes glass-out {
  from {
    opacity: 1;
  }
  to {
    opacity: 0;
  }
}

@keyframes glass-in {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes fade-in {
  from {
    opacity: 0;
  }
}

@keyframes fade-out {
  to {
    opacity: 0;
  }
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .flight-glass,
  .strip {
    animation-duration: 1ms !important;
  }
}
</style>
