<template>
  <div
    id="app"
    :class="{
      'user-select-none': userSelectNone,
      'window-hidden': windowHidden,
      'windows-compact-browse-prepared':
        compactBrowseEntranceState === 'prepared',
      'windows-compact-browse-entering':
        compactBrowseEntranceState === 'entering',
      'windows-compact-browse-origin-top':
        compactBrowseEntranceOrigin === 'top',
      'windows-compact-browse-origin-bottom':
        compactBrowseEntranceOrigin === 'bottom',
      'windows-compact-browse-reduced-motion': compactBrowseReducedMotion,
    }"
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
        :compact-expansion-exit="compactExpansionExit"
        :compact-expansion-origin="compactExpansionExitOrigin"
        :compact-expansion-height="compactExpansionExitHeight"
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
} from '@/services/compactWindow';
import {
  runStagedCompactExpansion,
  shouldAutoOpenLyricsOnResize,
} from '@/services/compactExpansionTransition';
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
      compactExpansionExit: false,
      compactExpansionExitOrigin: 'top' as 'top' | 'bottom',
      compactExpansionExitHeight: 0,
      windowsCompactExpansionBusy: false,
      suppressExpandedQueueAutoLyrics: false,
      compactBrowseEntranceState: 'idle' as 'idle' | 'prepared' | 'entering',
      compactBrowseEntranceOrigin: 'top' as 'top' | 'bottom',
      compactBrowseReducedMotion: false,
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
  },
  watch: {
    showLyrics(show: boolean) {
      if (
        show &&
        isWindows &&
        isTauriRuntime &&
        !this.autoOpenedLyrics &&
        !this.windowsCompactExpansionBusy
      ) {
        this.suppressExpandedQueueAutoLyrics = false;
      }
    },
    '$route.name'(name: string) {
      if (name !== 'next' && !this.windowsCompactExpansionBusy) {
        this.suppressExpandedQueueAutoLyrics = false;
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
        if (this.windowsCompactExpansionBusy) return;
        this.windowsCompactExpansionBusy = true;
        const previousRoute = this.$route.fullPath;
        const wasLyricsVisible = this.showLyrics;
        const wasAutoOpenedLyrics = this.autoOpenedLyrics;
        const wasSuppressingExpandedQueueAutoLyrics =
          this.suppressExpandedQueueAutoLyrics;
        const reducedMotion = window.matchMedia(
          '(prefers-reduced-motion: reduce)'
        ).matches;
        const restorePreviousState = async () => {
          if (this.$route.fullPath !== previousRoute) {
            try {
              await this.$router.push(previousRoute);
            } catch (_) {
              // The native window still needs to return to a usable compact state.
            }
          }
          if (wasLyricsVisible && !this.showLyrics) this.toggleLyrics();
          this.autoOpenedLyrics = wasAutoOpenedLyrics;
          this.suppressExpandedQueueAutoLyrics =
            wasSuppressingExpandedQueueAutoLyrics;
        };

        try {
          const expanded = await runStagedCompactExpansion(
            {
              prepareRoute: async () => {
                if (this.$route.name !== 'next') {
                  await this.$router.push({ name: 'next' });
                }
                if (this.$route.name !== 'next') {
                  throw new Error('Unable to prepare the next track page');
                }
                await this.$nextTick();
              },
              beginExit: () => {
                this.compactExpansionExitHeight = window.innerHeight;
                this.compactExpansionExit = true;
                this.autoOpenedLyrics = false;
                this.suppressExpandedQueueAutoLyrics = true;
                this.compactBrowseReducedMotion = reducedMotion;
                this.compactBrowseEntranceState = 'prepared';
                if (this.showLyrics) this.toggleLyrics();
              },
              expand: () =>
                expandCompactWindow({
                  onPrepared: origin => {
                    this.compactBrowseEntranceOrigin = origin;
                    this.compactExpansionExitOrigin = origin;
                  },
                }),
              beginEntrance: () => {
                this.compactBrowseEntranceState = 'entering';
              },
              finish: () => {
                this.compactExpansionExit = false;
                this.compactExpansionExitHeight = 0;
                this.compactBrowseEntranceState = 'idle';
                this.compactBrowseReducedMotion = false;
              },
            },
            { reducedMotion }
          );

          if (!expanded) {
            await restorePreviousState();
            return;
          }
          this.compactWindowExpanded = true;
        } catch (error) {
          console.warn('[compact-window] expansion transition failed', error);
          await restorePreviousState();
        } finally {
          this.compactExpansionExit = false;
          this.compactExpansionExitHeight = 0;
          this.compactBrowseEntranceState = 'idle';
          this.compactBrowseReducedMotion = false;
          this.windowsCompactExpansionBusy = false;
          // Reconcile a resize or Escape that returned to the bar during the entrance.
          this.handleMiniResize();
        }
        return;
      }

      if (!(await expandCompactWindow())) return;
      this.compactWindowExpanded = true;
      if (this.$route.name !== 'next') {
        await this.$router.push({ name: 'next' });
      }
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
      // The Esc pill follows the bar/browse split, not the lyrics routing.
      this.compactWindowExpanded =
        !isBarWindowSize(size) && hasRememberedBarFrame();
      const windowsFeatureActive = isWindows && isTauriRuntime;
      if (
        windowsFeatureActive &&
        this.suppressExpandedQueueAutoLyrics &&
        isBarWindowSize(size)
      ) {
        this.suppressExpandedQueueAutoLyrics = false;
      }
      const shouldAutoOpen = shouldAutoOpenLyricsOnResize(
        windowsFeatureActive,
        this.windowsCompactExpansionBusy,
        this.suppressExpandedQueueAutoLyrics,
        size
      );
      if (shouldAutoOpen && !this.showLyrics) {
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

#app.windows-compact-browse-prepared main,
#app.windows-compact-browse-prepared nav,
#app.windows-compact-browse-prepared .player,
#app.windows-compact-browse-entering main,
#app.windows-compact-browse-entering nav,
#app.windows-compact-browse-entering .player {
  will-change: transform, opacity;
}

#app.windows-compact-browse-prepared main,
#app.windows-compact-browse-prepared nav,
#app.windows-compact-browse-prepared .player {
  opacity: 0;
  transition: none !important;
}

#app.windows-compact-browse-prepared.windows-compact-browse-origin-top main,
#app.windows-compact-browse-prepared.windows-compact-browse-origin-top nav,
#app.windows-compact-browse-prepared.windows-compact-browse-origin-top .player {
  transform: translate3d(0, -12px, 0) scale(0.97);
  transform-origin: center top;
}

#app.windows-compact-browse-prepared.windows-compact-browse-origin-bottom main,
#app.windows-compact-browse-prepared.windows-compact-browse-origin-bottom nav,
#app.windows-compact-browse-prepared.windows-compact-browse-origin-bottom
  .player {
  transform: translate3d(0, 12px, 0) scale(0.97);
  transform-origin: center bottom;
}

#app.windows-compact-browse-entering main,
#app.windows-compact-browse-entering nav,
#app.windows-compact-browse-entering .player {
  opacity: 1;
  transform: translate3d(0, 0, 0) scale(1);
  transition: transform 420ms cubic-bezier(0.32, 0.72, 0, 1),
    opacity 420ms cubic-bezier(0.32, 0.72, 0, 1) !important;
}

#app.windows-compact-browse-entering nav {
  transition-delay: 8ms !important;
}

#app.windows-compact-browse-entering .player {
  transition-delay: 16ms !important;
}

#app.windows-compact-browse-reduced-motion.windows-compact-browse-prepared main,
#app.windows-compact-browse-reduced-motion.windows-compact-browse-prepared nav,
#app.windows-compact-browse-reduced-motion.windows-compact-browse-prepared
  .player {
  transform: none;
}

#app.windows-compact-browse-reduced-motion.windows-compact-browse-entering main,
#app.windows-compact-browse-reduced-motion.windows-compact-browse-entering nav,
#app.windows-compact-browse-reduced-motion.windows-compact-browse-entering
  .player {
  transform: none;
  transition: opacity 120ms cubic-bezier(0.23, 1, 0.32, 1) !important;
  transition-delay: 0ms !important;
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
