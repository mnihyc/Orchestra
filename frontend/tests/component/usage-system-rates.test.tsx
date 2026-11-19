/** The system catalogue handles an empty installation and names its file and origin in the reader's language. */
import userEvent from '@testing-library/user-event';
import { screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';

import type * as UsageApi from '@/api/usage';
import { SystemRates } from '@/features/usage/SystemRates';
import type { RateCatalogue, RateTable } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { show } from './support';

const fake = vi.hoisted(() => ({ catalogue: null as RateCatalogue | null, url: null as string | null, replace: vi.fn<typeof UsageApi.setCatalogue>() }));
vi.mock('@/api/usage', async (load) => ({
  ...await load<typeof UsageApi>(),
  nodeSettingsQuery: () => ({ queryKey: ['system-rates-settings'], queryFn: () => Promise.resolve([{ key: 'usage.catalogue_url', value: fake.url, own: null }]) }),
  setCatalogue: fake.replace,
  ratesQuery: () => ({ queryKey: ['system-rates-table'], queryFn: (): Promise<RateTable> => Promise.resolve({ models: [], catalogue: { on: true, scale: '1', as_of: '' } }) }),
  catalogueQuery: () => ({ queryKey: ['system-rates-catalogue'], queryFn: (): Promise<RateCatalogue | null> => Promise.resolve(fake.catalogue) }),
}));

beforeEach(() => { fake.catalogue = null; fake.url = null; fake.replace.mockReset(); });

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)(
  'shows no loaded catalogue without an error or a repeated upload label (%s, %s)', async (locale, theme) => {
    const previous = currentLocale();
    const oldTheme = document.documentElement.getAttribute('data-theme');
    await setLocale(locale);
    document.documentElement.setAttribute('data-theme', theme);
    const view = show(<SystemRates />);
    try {
      await screen.findByText(i18n.t('usage.rates.catalogue.none'), { exact: true });
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(screen.getAllByText(i18n.t('usage.rates.catalogue.none'), { exact: true })).toHaveLength(1);
      expect(screen.getByLabelText(i18n.t('usageAdmin.file'))).toHaveAttribute('type', 'file');
      expect(screen.getAllByText(i18n.t('usageAdmin.upload'), { exact: true })).toHaveLength(1);
      expect(screen.getByRole('button', { name: i18n.t('usage.rates.see') })).toBeEnabled();
    } finally {
      view.unmount();
      await setLocale(previous);
      if (oldTheme === null) document.documentElement.removeAttribute('data-theme');
      else document.documentElement.setAttribute('data-theme', oldTheme);
    }
  },
);

it('translates a loaded catalogue origin instead of showing the wire enum', async () => {
  const previous = currentLocale();
  await setLocale('zh-CN');
  fake.catalogue = { format: 1, as_of: '2026-10-10', unit: 'USD', origin: 'shipped', source: null,
    loaded_at: '2026-10-15T00:00:00Z', loaded_by: null, shipped_as_of: '2026-10-10', families: [] };
  const view = show(<SystemRates />);
  try {
    expect(await screen.findByText(/随本版本提供/)).toBeVisible();
    expect(view.container).not.toHaveTextContent('shipped');
    expect(screen.queryByText(i18n.t('usage.rates.catalogue.none'), { exact: true })).not.toBeInTheDocument();
  } finally { view.unmount(); await setLocale(previous); }
});


it('explains why Fetch now needs a source before making a request', async () => {
  await setLocale('en');
  const user = userEvent.setup(); const view = show(<SystemRates />);
  try {
    const fetch = await screen.findByRole('button', { name: 'Fetch now' });
    expect(fetch).toHaveAttribute('aria-disabled', 'true');
    await user.click(fetch); expect(fake.replace).not.toHaveBeenCalled();
    expect(fetch).toHaveAccessibleDescription('Set a catalogue URL first');
    expect(screen.getByRole('link', { name: 'Source' })).toHaveAttribute('href', '/admin/settings?setting=usage.catalogue_url');
  } finally { view.unmount(); }
});

it('shows the fetched catalogue and its outcome without a document reload', async () => {
  await setLocale('en'); fake.url = 'https://rates.example/catalogue.json';
  const after: RateCatalogue = { format: 1, as_of: '2026-11-01', unit: 'USD', origin: 'fetched', source: fake.url,
    loaded_at: '2026-11-02T00:00:00Z', loaded_by: null, shipped_as_of: '2026-10-10', families: [] };
  fake.replace.mockResolvedValue(after);
  const view = show(<SystemRates />);
  try {
    await userEvent.click(await screen.findByRole('button', { name: 'Fetch now' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Fetched 0 families');
    expect(screen.getByText(i18n.t('usage.rates.catalogueFrom', { date: after.as_of, origin: i18n.t('usageAdmin.origins.fetched') }))).toBeVisible();
    expect(fake.replace).toHaveBeenCalledExactlyOnceWith({ fetch: true });
  } finally { view.unmount(); }
});

it('says Nothing new when a fetch brings the same rates', async () => {
  await setLocale('en'); fake.url = 'https://rates.example/catalogue.json';
  fake.catalogue = { format: 1, as_of: '2026-11-01', unit: 'USD', origin: 'fetched', source: fake.url,
    loaded_at: '2026-11-01T00:00:00Z', loaded_by: null, shipped_as_of: '2026-10-10', families: [] };
  fake.replace.mockResolvedValue({ ...fake.catalogue, loaded_at: '2026-11-02T00:00:00Z' });
  const view = show(<SystemRates />);
  try {
    await userEvent.click(await screen.findByRole('button', { name: 'Fetch now' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Nothing new');
  } finally { view.unmount(); }
});
