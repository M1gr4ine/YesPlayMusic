import { createApp } from 'vue';
import { createI18n } from 'vue-i18n';
import TaskbarOverlayApp from '@/components/TaskbarOverlayApp.vue';
import { DEFAULT_LOCALE, localeMessages } from '@/locale/catalog';

const i18n = createI18n({
  legacy: false,
  locale: DEFAULT_LOCALE,
  fallbackLocale: DEFAULT_LOCALE,
  messages: localeMessages,
});

createApp(TaskbarOverlayApp).use(i18n).mount('#app');
