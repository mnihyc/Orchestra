/** Device-mode times follow this browser immediately, while a deliberate fixed zone stays fixed. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { adoptAppearance, useAppearance } from '@/lib/appearance';
import { browserTimeZone, formatTimeOfDay } from '@/lib/time';
import type * as Time from '@/lib/time';

vi.mock('@/lib/time', async (original) => ({ ...await original<typeof Time>(), browserTimeZone: vi.fn(() => 'Asia/Singapore') }));

const initial = useAppearance.getState();
const preferences = { theme: 'light', density: 'comfortable', locale: 'en', timezone: 'UTC', timezone_mode: 'device' } as const;
const noon = '2026-10-15T12:00:00Z';

beforeEach(() => { vi.mocked(browserTimeZone).mockReturnValue('Asia/Singapore'); });
afterEach(() => { useAppearance.setState(initial); });

describe('the zone a time is shown in', () => {
  it('ignores a stale stored UTC report in device mode, before any server answer can update it', () => {
    adoptAppearance(preferences);
    expect(useAppearance.getState()).toMatchObject({ timeZone: 'Asia/Singapore', timeZoneMode: 'device' });
    expect(formatTimeOfDay(noon, useAppearance.getState(), noon)).toBe('08:00 PM');
  });

  it('also ignores a report from another device and follows the current browser on the next adoption', () => {
    adoptAppearance({ ...preferences, timezone: 'America/Los_Angeles' });
    expect(formatTimeOfDay(noon, useAppearance.getState(), noon)).toBe('08:00 PM');
    vi.mocked(browserTimeZone).mockReturnValue('America/New_York');
    adoptAppearance({ ...preferences, timezone: 'America/Los_Angeles' });
    expect(formatTimeOfDay(noon, useAppearance.getState(), noon)).toBe('08:00 AM');
  });

  it('respects an explicitly fixed UTC zone on the same device', () => {
    adoptAppearance({ ...preferences, timezone_mode: 'fixed' });
    expect(useAppearance.getState()).toMatchObject({ timeZone: 'UTC', timeZoneMode: 'fixed' });
    expect(formatTimeOfDay(noon, useAppearance.getState(), noon)).toBe('12:00 PM');
  });

  it('keeps device mode current when another appearance preference changes', () => {
    adoptAppearance(preferences);
    vi.mocked(browserTimeZone).mockReturnValue('America/New_York');
    useAppearance.getState().set({ density: 'compact' });
    expect(useAppearance.getState()).toMatchObject({ density: 'compact', timeZone: 'America/New_York', timeZoneMode: 'device' });
  });
});
