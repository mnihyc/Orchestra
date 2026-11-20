/** The first shell uses only the device's presentation choices, before application services load. */
import type { Density, Locale, Theme } from '@/generated/api-types';
import { isLocale, nearestLocale } from '@/i18n/locales';

const STORAGE_KEY = 'orch.appearance';
export interface DeviceAppearance { theme: Theme; density: Density; locale: Locale }

export const darkPreferred = (): MediaQueryList | null =>
  typeof matchMedia === 'function' ? matchMedia('(prefers-color-scheme: dark)') : null;

export function resolveTheme(theme: Theme): 'light' | 'dark' {
  return theme === 'system' ? darkPreferred()?.matches ? 'dark' : 'light' : theme;
}

export function readDeviceAppearance(): DeviceAppearance {
  const fallback: DeviceAppearance = { theme: 'system', density: 'comfortable', locale: nearestLocale(typeof navigator === 'undefined' ? [] : navigator.languages) };
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null');
    if (typeof stored !== 'object' || stored === null) return fallback;
    const { theme, density, locale } = stored as Record<string, unknown>;
    return { theme: theme === 'light' || theme === 'dark' || theme === 'system' ? theme : fallback.theme,
      density: density === 'compact' || density === 'comfortable' ? density : fallback.density,
      locale: isLocale(locale) ? locale : fallback.locale };
  } catch { return fallback; }
}

export function applyDeviceAppearance(appearance: DeviceAppearance, resolvedTheme = resolveTheme(appearance.theme)): void {
  const root = document.documentElement;
  root.dataset.theme = resolvedTheme;
  root.dataset.density = appearance.density;
  root.lang = appearance.locale;
}

export function rememberDeviceAppearance({ theme, density, locale }: DeviceAppearance): void {
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify({ theme, density, locale })); }
  catch { /* A device that keeps nothing still shows the page. */ }
}
