/** Theme, density, language and time zone: what a device remembers of them, and how the page is told.
 *
 * These are a person's own preferences and follow them from the server once they are signed in. The
 * device keeps the last ones it was told, so that the pages before sign-in, and the first moment of a
 * reload, look the way their reader left them. Nothing else is kept here: it is how the page looks,
 * and never anything of a person's work.
 */
import { create } from 'zustand';

import type { Density, Locale, Me, Theme, TimezoneMode } from '@/generated/api-types';
import { isLocale, nearestLocale, setLocale } from '@/i18n';
import { browserTimeZone } from '@/lib/time';

/** Where the device keeps them. */
const STORAGE_KEY = 'orch.appearance';

export interface Appearance {
  theme: Theme;
  density: Density;
  locale: Locale;
  /** Device mode follows this browser; fixed mode follows the person's chosen zone. */
  timeZone: string;
  timeZoneMode: TimezoneMode;
}

interface AppearanceStore extends Appearance {
  /** The theme in force: `system` resolved by what the browser prefers now. */
  resolvedTheme: 'light' | 'dark';
  set: (change: Partial<Appearance>) => void;
}

const darkPreferred = (): MediaQueryList | null =>
  typeof matchMedia === 'function' ? matchMedia('(prefers-color-scheme: dark)') : null;

function resolve(theme: Theme): 'light' | 'dark' {
  if (theme !== 'system') return theme;
  return darkPreferred()?.matches ? 'dark' : 'light';
}

function remembered(): Appearance {
  const fallback: Appearance = {
    theme: 'system',
    density: 'comfortable',
    locale: nearestLocale(typeof navigator === 'undefined' ? [] : navigator.languages),
    timeZone: browserTimeZone(),
    timeZoneMode: 'device',
  };
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null');
    if (typeof stored !== 'object' || stored === null) return fallback;
    const { theme, density, locale } = stored as Record<string, unknown>;
    return {
      theme: theme === 'light' || theme === 'dark' || theme === 'system' ? theme : fallback.theme,
      density: density === 'compact' || density === 'comfortable' ? density : fallback.density,
      locale: isLocale(locale) ? locale : fallback.locale,
      timeZone: fallback.timeZone,
      timeZoneMode: fallback.timeZoneMode,
    };
  } catch {
    // Storage that is closed to the page, or holds something else: start from what the browser says.
    return fallback;
  }
}

function apply(appearance: Appearance, resolvedTheme: 'light' | 'dark'): void {
  const root = document.documentElement;
  root.dataset.theme = resolvedTheme;
  root.dataset.density = appearance.density;
  root.lang = appearance.locale;
}

function remember({ theme, density, locale }: Appearance): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ theme, density, locale }));
  } catch {
    // A device that keeps nothing still shows the page.
  }
}

export const useAppearance = create<AppearanceStore>((set, get) => {
  const initial = remembered();
  return {
    ...initial,
    resolvedTheme: resolve(initial.theme),
    set: (change) => {
      const before = get();
      const timeZoneMode = change.timeZoneMode ?? before.timeZoneMode;
      const next: Appearance = {
        theme: change.theme ?? before.theme,
        density: change.density ?? before.density,
        locale: change.locale ?? before.locale,
        timeZone: timeZoneMode === 'device' ? browserTimeZone() : change.timeZone ?? before.timeZone,
        timeZoneMode,
      };
      const resolvedTheme = resolve(next.theme);
      set({ ...next, resolvedTheme });
      apply(next, resolvedTheme);
      remember(next);
      if (next.locale !== before.locale) void setLocale(next.locale);
    },
  };
});

/** Writes the remembered appearance onto the page and follows the system's theme from then on. */
export function startAppearance(): Appearance {
  const state = useAppearance.getState();
  apply(state, state.resolvedTheme);
  darkPreferred()?.addEventListener('change', () => {
    const current = useAppearance.getState();
    if (current.theme !== 'system') return;
    const resolvedTheme = resolve('system');
    useAppearance.setState({ resolvedTheme });
    apply(current, resolvedTheme);
  });
  return state;
}

/** Takes the preferences of whoever signed in. */
export function adoptAppearance(me: Pick<Me, 'theme' | 'density' | 'locale' | 'timezone' | 'timezone_mode'>): void {
  useAppearance.getState().set({
    theme: me.theme,
    density: me.density,
    locale: me.locale,
    // The stored zone may be a report from a different device. Rendering never waits for our report.
    timeZone: me.timezone_mode === 'device' ? browserTimeZone() : me.timezone || browserTimeZone(),
    timeZoneMode: me.timezone_mode,
  });
}

/** The language and time zone that times and numbers are formatted in. */
export function useFormatContext(): { locale: Locale; timeZone: string } {
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  return { locale, timeZone };
}
