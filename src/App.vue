<template>
  <div
    id="app"
    :class="[
      {
        'user-select-none': userSelectNone,
        'window-hidden': windowHidden,
      },
      compactExpansionClasses,
    ]"
  >
    <Scrollbar
      v-show="!showLyrics"
      ref="scrollbar"
      @drag-state-change="userSelectNone = $event"
    />
    <Navbar
      v-show="showNavbar"
      ref="navbar"
      :compact-window-expanded="compactWindowExpanded"
      @restore-compact-window="restoreCompactWindow"
    />
    <main
      ref="main"
      data-scroll-container
      :style="{ overflow: enableScrolling ? 'auto' : 'hidden' }"
      @scroll="handleScroll"
    >
      <router-view v-slot="{ Component, route }">
        <keep-alive :max="4">
          <component
            :is="Component"
            v-if="route.meta['keepAlive']"
            :key="route.fullPath"
          />
        </keep-alive>
        <component
          :is="Component"
          v-if="!route.meta['keepAlive']"
          :key="route.fullPath"
        />
      </router-view>
    </main>
    <transition name="slide-up">
      <Player v-if="enablePlayer" v-show="showPlayer" ref="player" />
    </transition>
    <Toast />
    <ModalAddTrackToPlaylist v-if="isAccountLoggedIn" />
    <ModalNewPlaylist v-if="isAccountLoggedIn" />
    <ModalCloseApp
      :show="closePromptVisible"
      @cancel="cancelCloseChoice"
      @choose="resolveCloseChoice"
    />
    <transition v-if="enablePlayer" name="slide-up">
      <Lyrics
        v-show="showLyrics"
        :compact-exit="compactLyricsExit"
        @expand-compact-window="expandCompactWindow"
      />
    </transition>
  </div>
</template>

<script lang="ts">
import { defineComponent } from 'vue';
import ModalAddTrackToPlaylist from './components/ModalAddTrackToPlaylist.vue';
import ModalNewPlaylist from './components/ModalNewPlaylist.vue';
import ModalCloseApp from './components/ModalCloseApp.vue';
import Scrollbar from './components/Scrollbar.vue';
import Navbar from './components/Navbar.vue';
import Player from './components/Player.vue';
import Toast from './components/Toast.vue';
import { isAccountLoggedIn, isLooseLoggedIn } from '@/utils/auth';
import { refreshAccountData } from '@/services/accountBootstrap';
import Lyrics from './views/lyrics.vue';
import { mapActions, mapState } from 'pinia';
import { useAppStore } from '@/stores/app';
import { observeDocumentVisibility } from '@/utils/mediaLifecycle';
import { connectDesktopEvents } from '@/services/desktopBridge';
import { sendDesktop } from '@/services/desktopTransport';
import { isDesktopRuntime, isTauriRuntime } from '@/utils/runtime';
import {
  COMPACT_RESIZE_SETTLE_MS,
  expandCompactWindow,
  hasRememberedBarFrame,
  rememberCurrentCompactWindowFrame,
  restoreRememberedCompactWindowFrame,
  restoreCompactWindow,
  signalInitialWindowReady,
  type CompactExpansionOrigin,
} from '@/services/compactWindow';
import { isBarWindowSize, isMiniWindowSize } from '@/utils/miniWindow';
import {
  isEditableShortcutTarget,
  resolveRuntimeShortcutAction,
  runLocalShortcutAction,
} from '@/services/localShortcuts';
import { checkForAppUpdateInBackground } from '@/services/appUpdater';
import { isMac, isWindows } from '@/utils/platform';
import type { AppShell } from '@/types/appShell';
import type { RouteLocationRaw } from 'vue-router';
import { isLastfmCallbackLocation } from '@/services/lastfmAuth';

export default defineComponent({
  name: 'App',
  provide() {
    const appShell: AppShell = {
      restoreScrollPosition: () =>
        (
          this.$refs['scrollbar'] as InstanceType<typeof Scrollbar> | undefined
        )?.restorePosition(),
      scrollMainTo: (optionsOrX?: ScrollToOptions | number, y?: number) => {
        const main = this.$refs['main'] as HTMLElement | undefined;
        if (!main) return;
        if (typeof optionsOrX === 'number') main.scrollTo(optionsOrX, y ?? 0);
        else main.scrollTo(optionsOrX);
      },
    };
    return {
      appShell,
    };
  },
  components: {
    Navbar,
    Player,
    Toast,
    ModalAddTrackToPlaylist,
    ModalNewPlaylist,
    ModalCloseApp,
    Lyrics,
    Scrollbar,
  },
  data() {
    return {
      isDesktop: isDesktopRuntime,
      isLastfmCallback: isLastfmCallbackLocation(window.location),
      userSelectNone: false,
      autoOpenedLyrics: false, // Restore lyrics only when compact mode opened it.
      compactExpansion: null as {
        phase: 'preparing' | 'leaving' | 'entering';
        origin: CompactExpansionOrigin;
        height: number;
      } | null,
      // Narrowing the queue the user expanded into must not flip to lyrics.
      keepExpandedQueue: false,
      compactWindowExpanded:
        !isBarWindowSize({
          width: window.innerWidth,
          height: window.innerHeight,
        }) && hasRememberedBarFrame(),
      compactResizeTimer: null as ReturnType<typeof setTimeout> | null,
      compactWindowMemoryReady: !isDesktopRuntime,
      windowHidden: document.hidden,
      visibilityCleanup: null as (() => void) | null,
      desktopEventsCleanup: null as Promise<() => void> | null,
      closePromptVisible: false,
    };
  },
  computed: {
    ...mapState(useAppStore, [
      'showLyrics',
      'settings',
      'player',
      'enableScrolling',
    ]),
    isAccountLoggedIn() {
      return isAccountLoggedIn();
    },
    showPlayer() {
      return (
        [
          'mv',
          'loginUsername',
          'login',
          'loginAccount',
          'lastfmCallback',
        ].includes(String(this.$route.name ?? '')) === false
      );
    },
    enablePlayer() {
      return this.player.enabled && this.$route.name !== 'lastfmCallback';
    },
    showNavbar() {
      return this.$route.name !== 'lastfmCallback';
    },
    compactExpansionClasses(): string[] {
      const expansion = this.compactExpansion;
      return expansion
        ? [
            `compact-expansion-${expansion.phase}`,
            `compact-expansion-from-${expansion.origin}`,
          ]
        : [];
    },
    compactLyricsExit() {
      const expansion = this.compactExpansion;
      return expansion && expansion.phase !== 'preparing'
        ? { origin: expansion.origin, height: expansion.height }
        : null;
    },
  },
  watch: {
    '$route.name'(name: unknown) {
      if (name !== 'next' && !this.compactExpansion) {
        this.keepExpandedQueue = false;
      }
    },
  },
  created() {
    if (this.isLastfmCallback) return;
    if (this.isDesktop && !this.isLastfmCallback) {
      this.desktopEventsCleanup = connectDesktopEvents({
        pushRoute: path => this.$router.push(path as RouteLocationRaw),
        focusSearch: () =>
          (this.$refs['navbar'] as InstanceType<typeof Navbar>).focusSearch(),
        goHistory: where =>
          (this.$refs['navbar'] as InstanceType<typeof Navbar>).go(where),
        goToNextTracksPage: () =>
          (
            this.$refs['player'] as InstanceType<typeof Player>
          ).goToNextTracksPage(),
        requestCloseChoice: () => (this.closePromptVisible = true),
      });
    }
    window.addEventListener('keydown', this.handleKeydown);
    window.addEventListener('resize', this.handleMiniResize);
    this.visibilityCleanup = observeDocumentVisibility(
      document,
      hidden => (this.windowHidden = hidden)
    );
    if (this.isDesktop && !this.isLastfmCallback) {
      void this.initializeCompactWindowMemory();
    } else {
      this.handleMiniResize();
    }
    if (!this.isLastfmCallback) this.fetchData();
    if (this.isDesktop && !this.isLastfmCallback)
      void this.checkForUpdateOnStartup();
  },
  beforeUnmount() {
    window.removeEventListener('keydown', this.handleKeydown);
    window.removeEventListener('resize', this.handleMiniResize);
    if (this.compactResizeTimer !== null) clearTimeout(this.compactResizeTimer);
    this.visibilityCleanup?.();
    this.desktopEventsCleanup?.then(cleanup => cleanup());
  },
  methods: {
    ...mapActions(useAppStore, [
      'toggleLyrics',
      'fetchLikedSongs',
      'fetchLikedSongsWithDetails',
      'fetchLikedPlaylist',
      'fetchUserProfile',
      'likeATrack',
      'showToast',
    ]),
    async checkForUpdateOnStartup() {
      const result = await checkForAppUpdateInBackground();
      if (result?.status !== 'available') return;
      this.showToast(
        String(
          this.$t('settings.updater.available', {
            version: result.version,
          })
        )
      );
    },
    async initializeCompactWindowMemory() {
      try {
        const restored = await restoreRememberedCompactWindowFrame();
        this.compactWindowExpanded =
          restored?.mode === 'browse' && hasRememberedBarFrame();
      } catch (error) {
        // Keep resizing usable after a one-off restore failure.
        console.warn('[compact-window] startup restore failed', error);
      } finally {
        // Persist only after restoring logical dimensions.
        this.compactWindowMemoryReady = true;
        this.handleMiniResize();
        await this.$nextTick();
        try {
          await signalInitialWindowReady();
        } catch (error) {
          console.warn('[compact-window] readiness signal failed', error);
        }
      }
    },
    async resolveCloseChoice(payload: {
      action: 'exit' | 'minimizeToTray';
      remember: boolean;
    }) {
      this.closePromptVisible = false;
      await sendDesktop('resolveCloseChoice', payload);
    },
    async cancelCloseChoice() {
      this.closePromptVisible = false;
      await sendDesktop('cancelCloseChoice');
    },
    async expandCompactWindow() {
      if (isWindows && isTauriRuntime) {
        await this.expandCompactWindowAnimated();
        return;
      }
      if (!(await expandCompactWindow())) return;
      this.compactWindowExpanded = true;
      if (this.$route.name !== 'next') {
        await this.$router.push({ name: 'next' });
      }
    },
    // Fade the bar out, resize natively, then reveal the queue from the bar edge.
    async expandCompactWindowAnimated() {
      if (this.compactExpansion) return;
      const previousRoute = this.$route.fullPath;
      const lyricsWereVisible = this.showLyrics;
      const lyricsWereAutoOpened = this.autoOpenedLyrics;
      const reducedMotion = window.matchMedia(
        '(prefers-reduced-motion: reduce)'
      ).matches;
      const wait = (ms: number) =>
        new Promise<void>(resolve => setTimeout(resolve, ms));
      this.compactExpansion = {
        phase: 'preparing',
        origin: 'top',
        height: window.innerHeight,
      };
      let expanded = false;
      try {
        // Render the queue before the window grows into it.
        if (this.$route.name !== 'next') {
          await this.$router.push({ name: 'next' });
        }
        expanded =
          this.$route.name === 'next' &&
          (await expandCompactWindow({
            beforeResize: async origin => {
              this.compactExpansion = {
                phase: 'leaving',
                origin,
                height: window.innerHeight,
              };
              this.autoOpenedLyrics = false;
              if (this.showLyrics) this.toggleLyrics();
              if (!reducedMotion) await wait(80);
            },
          }));
        if (expanded) {
          this.keepExpandedQueue = true;
          this.compactWindowExpanded = true;
          await new Promise(resolve =>
            requestAnimationFrame(() => requestAnimationFrame(resolve))
          );
          this.compactExpansion.phase = 'entering';
          await wait(reducedMotion ? 120 : 440);
        }
      } catch (error) {
        console.warn('[compact-window] expansion failed', error);
      } finally {
        this.compactExpansion = null;
      }
      if (!expanded) {
        if (this.$route.fullPath !== previousRoute) {
          await this.$router.push(previousRoute).catch(() => undefined);
        }
        if (lyricsWereVisible && !this.showLyrics) this.toggleLyrics();
        this.autoOpenedLyrics = lyricsWereAutoOpened;
      }
      // Reconcile resizes and Escape presses that arrived mid-transition.
      this.handleMiniResize();
    },
    async restoreCompactWindow() {
      if (!(await restoreCompactWindow())) return;
      this.compactWindowExpanded = false;
    },
    // Open the lyrics player when the window becomes narrow or short.
    handleMiniResize() {
      const size = {
        width: window.innerWidth,
        height: window.innerHeight,
      };
      const isMini = isMiniWindowSize(size);
      const isBar = isBarWindowSize(size);
      // The Esc pill follows the bar/browse split, not the lyrics routing.
      this.compactWindowExpanded = !isBar && hasRememberedBarFrame();
      if (isBar) this.keepExpandedQueue = false;
      if (
        isMini &&
        !this.showLyrics &&
        !this.compactExpansion &&
        !this.keepExpandedQueue
      ) {
        this.toggleLyrics();
        this.autoOpenedLyrics = true;
      } else if (!isMini && this.showLyrics && this.autoOpenedLyrics) {
        this.toggleLyrics();
        this.autoOpenedLyrics = false;
      }
      this.scheduleCompactWindowMemory();
    },
    scheduleCompactWindowMemory() {
      if (!this.isDesktop || !this.compactWindowMemoryReady) return;
      if (this.compactResizeTimer !== null)
        clearTimeout(this.compactResizeTimer);
      // Persist only the settled layout mode.
      this.compactResizeTimer = setTimeout(async () => {
        const remembered = await rememberCurrentCompactWindowFrame();
        if (!remembered) return;
        this.compactWindowExpanded =
          remembered.mode === 'browse' && hasRememberedBarFrame();
      }, COMPACT_RESIZE_SETTLE_MS);
    },
    handleKeydown(e: KeyboardEvent) {
      if (e.defaultPrevented) return;
      if (e.code === 'Escape' && this.compactWindowExpanded) {
        e.preventDefault();
        void this.restoreCompactWindow();
        return;
      }
      if (isEditableShortcutTarget(e.target)) return;
      const action = resolveRuntimeShortcutAction(
        this.settings.shortcuts,
        e,
        isMac,
        this.isDesktop
      );
      if (!action) return;

      e.preventDefault();
      if (action === 'play' && this.$route.name === 'mv') return;
      const player = this.player;
      runLocalShortcutAction(action, {
        isPersonalFM: player.isPersonalFM,
        get volume() {
          return player.volume;
        },
        set volume(value: number) {
          player.volume = value;
        },
        currentTrackId: player.currentTrack.id,
        playOrPause: () => player.playOrPause(),
        playNextFMTrack: () => player.playNextFMTrack(),
        playNextTrack: () => player.playNextTrack(),
        playPrevTrack: () => player.playPrevTrack(),
        likeTrack: id => this.likeATrack(id),
        minimize: () => sendDesktop('minimize'),
      });
    },
    async fetchData() {
      if (!isLooseLoggedIn()) return;
      try {
        await refreshAccountData({
          accountLoggedIn: isAccountLoggedIn(),
          fetchUserProfile: () => this.fetchUserProfile(),
          fetchLikedSongs: () => this.fetchLikedSongs(),
          fetchLikedPlaylist: () => this.fetchLikedPlaylist(),
          fetchLikedSongsWithDetails: () => this.fetchLikedSongsWithDetails(),
        });
      } catch (error) {
        console.warn('[account] Unable to refresh account data', error);
      }
      // Load large library collections only when the library opens.
    },
    handleScroll() {
      (
        this.$refs['scrollbar'] as InstanceType<typeof Scrollbar>
      ).handleScroll();
    },
  },
});
</script>

<style lang="scss">
#app {
  width: 100%;
  transition: all 0.4s;
}

// Windows compact expansion: the queue rises out of the bar's edge.
#app.compact-expansion-leaving,
#app.compact-expansion-entering {
  main,
  nav,
  .player {
    will-change: transform, opacity;
  }
}

#app.compact-expansion-leaving {
  main,
  nav,
  .player {
    opacity: 0;
    transform: translate3d(0, -12px, 0) scale(0.97);
    transform-origin: center top;
    transition: none !important;
  }

  &.compact-expansion-from-bottom {
    main,
    nav,
    .player {
      transform: translate3d(0, 12px, 0) scale(0.97);
      transform-origin: center bottom;
    }
  }
}

#app.compact-expansion-entering {
  main,
  nav,
  .player {
    transition: transform 420ms cubic-bezier(0.32, 0.72, 0, 1),
      opacity 420ms cubic-bezier(0.32, 0.72, 0, 1) !important;
  }

  nav {
    transition-delay: 8ms !important;
  }

  .player {
    transition-delay: 16ms !important;
  }
}

@media (prefers-reduced-motion: reduce) {
  #app.compact-expansion-leaving {
    main,
    nav,
    .player {
      transform: none !important;
    }
  }

  #app.compact-expansion-entering {
    main,
    nav,
    .player {
      transition: opacity 120ms cubic-bezier(0.23, 1, 0.32, 1) !important;
    }
  }
}

// Pause visual effects while the hidden WebView keeps running.
#app.window-hidden *,
#app.window-hidden *::before,
#app.window-hidden *::after {
  animation-play-state: paused !important;
}

main {
  position: fixed;
  top: 0;
  bottom: 0;
  right: 0;
  left: 0;
  overflow: auto;
  padding: 64px 10vw 96px 10vw;
  box-sizing: border-box;
  scrollbar-width: none; // firefox
}

@media (max-width: 1336px) {
  main {
    padding: 64px 5vw 96px 5vw;
  }
}

main::-webkit-scrollbar {
  width: 0px;
}

.slide-up-enter-active,
.slide-up-leave-active {
  transition: transform 0.4s;
}
.slide-up-enter-from,
.slide-up-leave-to {
  transform: translateY(100%);
}
</style>
