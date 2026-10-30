/** A person finds a zone by name or current offset, or follows this device without waiting for a save. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import { useQuery } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { meQuery, updateMe } from '@/api/auth';
import type * as AuthApi from '@/api/auth';
import type * as Time from '@/lib/time';
import { LookRows } from '@/features/settings/personal/Profile';
import type { Me } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { adoptAppearance, useAppearance } from '@/lib/appearance';
import { TimeZonePicker, type TimeZoneValue } from '@/ui/TimeZonePicker';
import { me } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/lib/time', async (original) => ({
  ...await original<typeof Time>(),
  browserTimeZone: () => 'Asia/Singapore',
  useNow: () => Date.parse('2026-10-15T12:00:00Z'),
}));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(), updateMe: vi.fn() }));

// Include a fractional neighbour to prove that +8 means the current offset, not a text prefix.
const ZONES = ['America/New_York', 'Asia/Kolkata', 'Asia/Shanghai', 'Asia/Singapore', 'Australia/Eucla', 'Europe/London', 'Pacific/Honolulu'];
const initialAppearance = useAppearance.getState();

/** Interface observes this cache: an acknowledged preference must reach its controls too. */
function AppearanceRows({ initial }: { initial: Me }) {
  const person = useQuery({ ...meQuery, initialData: initial, enabled: false });
  return <LookRows me={person.data} />;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps Interface appearance labels visible and options short in ${locale}/${theme}`, async () => {
    await setLocale(locale);
    const stored = me(1, 'ada', 'Ada', { locale, theme, density: 'comfortable', timezone: 'UTC', timezone_mode: 'fixed' });
    adoptAppearance(stored);
    const view = show(<AppearanceRows initial={stored} />);
    expect(screen.getByText(i18n.t('preferences.theme'), { exact: true })).toBeVisible();
    expect(screen.getByText(i18n.t('preferences.density'), { exact: true })).toBeVisible();
    const themeOptions = screen.getByLabelText(i18n.t('preferences.theme'));
    expect(within(themeOptions).getByText(i18n.t('profile.look.themeDevice'), { exact: true })).toBeVisible();
    expect(themeOptions).not.toHaveTextContent(i18n.t('choiceCatalog.theme.system.description'));
    const density = screen.getByLabelText(i18n.t('preferences.density'));
    expect(within(density).getByText(i18n.t('choiceCatalog.density.comfortable.name'), { exact: true })).toBeVisible();
    expect(density).not.toHaveTextContent(i18n.t('choiceCatalog.density.comfortable.description'));
    view.unmount();
    await setLocale('en');
  });
}

function Picker({ onChoose }: { onChoose: (choice: TimeZoneValue) => void }) {
  const [value, setValue] = useState<TimeZoneValue>({ timeZone: 'Europe/London', mode: 'fixed' });
  return <TimeZonePicker label="Time zone" value={value.timeZone} mode={value.mode} onValueChange={(choice) => { setValue(choice); onChoose(choice); }} />;
}

beforeEach(async () => {
  await setLocale('en');
  vi.spyOn(Intl, 'supportedValuesOf').mockReturnValue(ZONES);
  vi.mocked(updateMe).mockReset();
});
afterEach(async () => {
  cleanup();
  useAppearance.setState(initialAppearance);
  await setLocale('en');
});

describe('the searchable time zone choice', () => {
  it('puts this device, the zone in use and UTC first, then groups names by region with their current times', async () => {
    const user = userEvent.setup();
    show(<Picker onChoose={vi.fn()} />);
    const field = screen.getByRole('combobox', { name: 'Time zone' });
    expect(field).toHaveValue('Europe/London');
    expect(field).not.toHaveAttribute('aria-describedby');
    expect(screen.queryByText('Search by city, region, abbreviation or UTC offset, such as +8.')).not.toBeInTheDocument();
    await user.click(field);
    expect(field).toHaveAttribute('placeholder', 'City, region, abbreviation or +8');
    const quick = screen.getByRole('group', { name: 'Quick choices' });
    expect(within(quick).getAllByRole('option')).toHaveLength(3);
    expect(within(quick).getAllByRole('option')[0]).toHaveAccessibleName('This device');
    expect(within(quick).getAllByRole('option')[1]).toHaveAccessibleName('Europe/London');
    expect(within(quick).getAllByRole('option')[2]).toHaveAccessibleName('UTC');
    expect(screen.getByRole('option', { name: 'This device' })).toHaveAccessibleDescription(/Asia\/Singapore, UTC\+08:00 · 20:00/);
    expect(screen.getByRole('option', { name: 'Europe/London' })).toHaveAccessibleDescription(/UTC\+01:00 · 13:00/);
    expect(within(screen.getByRole('group', { name: 'Asia' })).getByRole('option', { name: 'Asia/Shanghai' })).toHaveAccessibleDescription(/UTC\+08:00 · 20:00/);
    expect(screen.getByRole('option', { name: 'UTC' })).toHaveAccessibleDescription(/UTC\+00:00 · 12:00/);
  });

  it.each(['+8', 'UTC+8'])('finds exact current offsets when a person types %s', async (query) => {
    const user = userEvent.setup();
    show(<Picker onChoose={vi.fn()} />);
    await user.type(screen.getByRole('combobox', { name: 'Time zone' }), query);
    expect(screen.getByRole('option', { name: 'Asia/Singapore' })).toBeVisible();
    expect(screen.getByRole('option', { name: 'Asia/Shanghai' })).toBeVisible();
    expect(screen.queryByRole('option', { name: 'Europe/London' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'Australia/Eucla' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'Pacific/Honolulu' })).not.toBeInTheDocument();
  });

  it('finds a city with spaces, keeps the offset out of its accessible name, and chooses it by Enter', async () => {
    const user = userEvent.setup(), choose = vi.fn();
    show(<Picker onChoose={choose} />);
    const field = screen.getByRole('combobox', { name: 'Time zone' });
    await user.type(field, 'new york');
    const city = screen.getByRole('option', { name: 'America/New York' });
    expect(city).toHaveAccessibleDescription(/UTC-04:00 · 08:00/);
    expect(screen.getAllByRole('option')).toHaveLength(1);
    await user.keyboard('{Enter}');
    expect(choose).toHaveBeenCalledWith({ timeZone: 'America/New_York', mode: 'fixed' });
    expect(field).toHaveValue('America/New York');
    expect(field).toHaveFocus();
  });

  it('also finds a region and the abbreviation in force on that day', async () => {
    const user = userEvent.setup();
    show(<Picker onChoose={vi.fn()} />);
    const field = screen.getByRole('combobox', { name: 'Time zone' });
    await user.type(field, 'Asia');
    expect(screen.getByRole('option', { name: 'Asia/Kolkata' })).toBeVisible();
    expect(screen.queryByRole('option', { name: 'America/New York' })).not.toBeInTheDocument();
    await user.clear(field);
    await user.type(field, 'EDT');
    expect(screen.getByRole('option', { name: 'America/New York' })).toBeVisible();
    expect(screen.getAllByRole('option')).toHaveLength(1);
    await user.clear(field);
    await user.type(field, 'SGT');
    expect(screen.getByRole('option', { name: 'Asia/Singapore' })).toBeVisible();
    expect(screen.queryByRole('option', { name: 'Asia/Shanghai' })).not.toBeInTheDocument();
    await user.clear(field);
    await user.type(field, '+5:30');
    expect(screen.getByRole('option', { name: 'Asia/Kolkata' })).toHaveAccessibleDescription(/UTC\+05:30 · 17:30/);
    expect(screen.getAllByRole('option')).toHaveLength(1);
  });

  it('reaches the final group and this device by keyboard, while Escape preserves the existing choice', async () => {
    const user = userEvent.setup(), choose = vi.fn();
    show(<><Picker onChoose={choose} /><button>Next setting</button></>);
    await user.tab();
    const field = screen.getByRole('combobox', { name: 'Time zone' });
    expect(field).toHaveFocus();
    await user.keyboard('{End}');
    expect(screen.getByRole('option', { name: 'Pacific/Honolulu', selected: true })).toBeVisible();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(field).toHaveValue('Europe/London');
    expect(choose).not.toHaveBeenCalled();
    await user.keyboard('{ArrowDown}{Home}{Enter}');
    expect(choose).toHaveBeenCalledWith({ timeZone: 'Asia/Singapore', mode: 'device' });
    expect(field).toHaveValue('This device (Asia/Singapore, UTC+08:00)');
    await user.tab();
    expect(screen.getByRole('button', { name: 'Next setting' })).toHaveFocus();
  });

  it('explains an unmatched search without choosing a substitute', async () => {
    const user = userEvent.setup(), choose = vi.fn();
    show(<Picker onChoose={choose} />);
    await user.type(screen.getByRole('combobox', { name: 'Time zone' }), 'no such city');
    expect(screen.getByText('No time zone matches. Try a city or UTC offset.')).toBeVisible();
    await user.keyboard('{Enter}');
    expect(choose).not.toHaveBeenCalled();
  });

  it('offers the same named device choice and translated region search in Chinese', async () => {
    await setLocale('zh-CN');
    const user = userEvent.setup(), choose = vi.fn();
    show(<TimeZonePicker label="时区" value="UTC" mode="fixed" onValueChange={choose} />);
    const field = screen.getByRole('combobox', { name: '时区' });
    await user.click(field);
    expect(screen.getByRole('option', { name: '此设备' })).toHaveAccessibleDescription(/Asia\/Singapore，UTC\+08:00/);
    await user.type(field, '亚洲');
    expect(screen.getByRole('option', { name: 'Asia/Shanghai' })).toBeVisible();
    expect(screen.queryByRole('option', { name: 'America/New York' })).not.toBeInTheDocument();
    await user.clear(field);
    await user.click(screen.getByRole('option', { name: '此设备' }));
    expect(choose).toHaveBeenCalledWith({ timeZone: 'Asia/Singapore', mode: 'device' });
  });
});

describe('the profile appearance control', () => {
  it.each(['en', 'zh-CN'] as const)('keeps theme and density labelled with short option names in %s, without option guides', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale), user = userEvent.setup();
    const stored = me(1, 'ada', 'Ada', { locale, theme: 'system' });
    adoptAppearance(stored);
    vi.mocked(updateMe).mockResolvedValue({ ...stored, theme: 'dark' });
    show(<AppearanceRows initial={stored} />);
    const theme = screen.getByRole('radiogroup', { name: t('preferences.theme') });
    expect(within(theme).getByRole('radio', { name: t('profile.look.themeDevice') })).toBeChecked();
    const density = screen.getByRole('radiogroup', { name: t('preferences.density') });
    for (const group of [theme, density]) for (const option of within(group).getAllByRole('radio')) expect(option).not.toHaveAttribute('aria-describedby');
    expect(screen.queryByText(t('choiceCatalog.theme.system.description'))).not.toBeInTheDocument();
    expect(screen.queryByText(t('choiceCatalog.density.comfortable.description'))).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Guide|指南/ })).not.toBeInTheDocument();
    await user.click(within(theme).getByRole('radio', { name: t('choiceCatalog.theme.dark.name') }));
    expect(updateMe).toHaveBeenCalledExactlyOnceWith({ theme: 'dark' });
    await waitFor(() => expect(within(theme).getByRole('radio', { name: t('choiceCatalog.theme.dark.name') })).toBeChecked());
  });

  it('writes device mode explicitly and shows the browser zone while the real save is still pending', async () => {
    const stored = me(1, 'ada', 'Ada', { timezone: 'UTC', timezone_mode: 'fixed' });
    let finish: ((updated: Me) => void) | undefined;
    vi.mocked(updateMe).mockImplementation(() => new Promise<Me>((resolve) => { finish = resolve; }));
    adoptAppearance(stored);
    const user = userEvent.setup();
    show(<AppearanceRows initial={stored} />);
    await user.click(screen.getByRole('combobox', { name: 'Time zone' }));
    await user.click(screen.getByRole('option', { name: 'This device' }));
    expect(updateMe).toHaveBeenCalledWith({ timezone: 'Asia/Singapore', timezone_mode: 'device' });
    expect(useAppearance.getState()).toMatchObject({ timeZone: 'Asia/Singapore', timeZoneMode: 'device' });
    expect(screen.getByRole('combobox', { name: 'Time zone' })).toHaveValue('This device (Asia/Singapore, UTC+08:00)');
    await act(async () => { finish?.({ ...stored, timezone: 'Asia/Singapore', timezone_mode: 'device' }); await Promise.resolve(); });
  });

  it('keeps a refused fixed draft while formatting in the confirmed device zone, and can choose this device again', async () => {
    const stored = me(1, 'ada', 'Ada', { timezone: 'UTC', timezone_mode: 'device' });
    let refuse: ((error: Error) => void) | undefined;
    vi.mocked(updateMe).mockImplementation(() => new Promise<Me>((_resolve, reject) => { refuse = reject; }));
    adoptAppearance(stored);
    const user = userEvent.setup();
    show(<AppearanceRows initial={stored} />);
    await user.type(screen.getByRole('combobox', { name: 'Time zone' }), 'London');
    await user.click(screen.getByRole('option', { name: 'Europe/London' }));
    expect(updateMe).toHaveBeenCalledWith({ timezone: 'Europe/London', timezone_mode: 'fixed' });
    expect(useAppearance.getState()).toMatchObject({ timeZone: 'Europe/London', timeZoneMode: 'fixed' });
    await act(async () => { refuse?.(new Error('Save refused')); await Promise.resolve(); });
    await waitFor(() => expect(useAppearance.getState()).toMatchObject({ timeZone: 'Asia/Singapore', timeZoneMode: 'device' }));
    expect(await screen.findByRole('alert')).toBeVisible();
    const field = screen.getByRole('combobox', { name: 'Time zone' });
    expect(field).toHaveValue('Europe/London');
    await waitFor(() => expect(field).toBeEnabled());
    expect(screen.getByRole('button', { name: i18n.t('common.tryAgain') })).toBeEnabled();
    expect(updateMe).toHaveBeenCalledTimes(1);
    vi.mocked(updateMe).mockResolvedValue({ ...stored, timezone: 'Asia/Singapore', timezone_mode: 'device' });
    await user.click(field);
    await user.click(screen.getByRole('option', { name: 'This device' }));
    await waitFor(() => expect(updateMe).toHaveBeenLastCalledWith({ timezone: 'Asia/Singapore', timezone_mode: 'device' }));
    expect(screen.getByRole('combobox', { name: 'Time zone' })).toHaveValue('This device (Asia/Singapore, UTC+08:00)');
    expect(useAppearance.getState()).toMatchObject({ timeZone: 'Asia/Singapore', timeZoneMode: 'device' });
  });
});
