/** Gallery words are loaded with the development gallery and are absent from the application catalogues. */
import { i18n } from '@/i18n';
import english from './dev-en.json';
import chinese from './dev-zh-CN.json';

export function installDevCatalogue(): void {
  i18n.addResourceBundle('en', 'translation', english, true, true);
  i18n.addResourceBundle('zh-CN', 'translation', chinese, true, true);
}
