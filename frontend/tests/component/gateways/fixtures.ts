import { setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
export { catalogue, connector, tunnel } from '../../doubles/gateways';
export const modes = (['en', 'zh-CN'] as const).flatMap((locale) => (['light', 'dark'] as const).flatMap((theme) =>
  ([400, 1440] as const).map((width) => ({ locale, theme, width }))));
const appearance = useAppearance.getState();
const beforeTheme = document.documentElement.dataset.theme;
const beforeDark = document.documentElement.classList.contains('dark');
const beforeWidth = window.innerWidth;
export async function look(mode: (typeof modes)[number]) {
  await setLocale(mode.locale);
  useAppearance.setState({ theme: mode.theme, resolvedTheme: mode.theme });
  document.documentElement.dataset.theme = mode.theme;
  document.documentElement.classList.toggle('dark', mode.theme === 'dark');
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: mode.width });
  window.dispatchEvent(new Event('resize'));
}
export async function restore() {
  useAppearance.setState(appearance);
  if (beforeTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = beforeTheme;
  document.documentElement.classList.toggle('dark', beforeDark);
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: beforeWidth });
  await setLocale('en');
}
