/** Theme, density, language and time zone: what a device remembers of them, and how the page is told.
 *
 * These are a person's own preferences and follow them from the server once they are signed in. The
 * device keeps the last ones it was told, so that the pages before sign-in, and the first moment of a
 * reload, look the way their reader left them. Nothing else is kept here: it is how the page looks,
 * and never anything of a person's work.
 */
import { create } from 'zustand';

import type { Density, Locale, Me, Theme, TimezoneMode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { applyDeviceAppearance as apply, darkPreferred, readDeviceAppearance, rememberDeviceAppearance as remember, resolveTheme as resolve } from './deviceAppearance';
import { browserTimeZone } from '@/lib/time';

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

export const useAppearance = create<AppearanceStore>((set, get) => {
  const initial: Appearance = { ...readDeviceAppearance(), timeZone: browserTimeZone(), timeZoneMode: 'device' };
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
      if (next.locale !== before.locale && i18n.isInitialized && next.locale !== i18n.language) void setLocale(next.locale);
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
