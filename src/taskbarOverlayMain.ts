import { createApp } from 'vue';
import { createI18n } from 'vue-i18n';
import TaskbarStrip from '@/components/TaskbarStrip.vue';
import { DEFAULT_LOCALE, localeMessages } from '@/locale/catalog';

// Entry of the Windows taskbar strip: no store, router or player. The
// player page mirrors playback into it and switches its locale.
const i18n = createI18n({
  legacy: false,
  locale: DEFAULT_LOCALE,
  fallbackLocale: DEFAULT_LOCALE,
  messages: localeMessages,
});

createApp(TaskbarStrip).use(i18n).mount('#app');
