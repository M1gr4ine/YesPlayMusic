import type { MessageKey } from '@/locale/catalog';

type MessageTranslator = (key: MessageKey) => string;

const locale = {
  t(key: MessageKey, translate: MessageTranslator): string {
    return translate(key);
  },
};

export default locale;
