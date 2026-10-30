/** An absent catalogue is a successful null read; a broken route remains a visible error. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { CatalogueDialog } from '@/features/usage/Catalogue';
import type { RateCatalogue } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { show } from './support';

const loaded: RateCatalogue = {
  format: 1, as_of: '2026-10-10', unit: 'USD', origin: 'shipped', source: null,
  loaded_at: '2026-10-15T00:00:00Z', loaded_by: null, shipped_as_of: '2026-10-10',
  families: [{ match: 'example-*', vendor: 'Example', as_of: '2026-10-10', source: 'https://example.test/pricing',
    rate: { input_per_mtok: '1', cached_input_per_mtok: '0.1', output_per_mtok: '5' } }],
};

afterEach(() => { vi.unstubAllGlobals(); });

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)(
  'shows the ordinary empty read once without an error or an unusable lookup (%s, %s)', async (locale, theme) => {
    const previous = currentLocale();
    const oldTheme = document.documentElement.getAttribute('data-theme');
    await setLocale(locale);
    document.documentElement.setAttribute('data-theme', theme);
    const fetch = vi.fn(() => Promise.resolve(Response.json(null)));
    vi.stubGlobal('fetch', fetch);
    const view = show(<CatalogueDialog open onOpenChange={() => undefined} />);
    try {
      const dialog = screen.getByRole('dialog', { name: i18n.t('usage.rates.catalogue.title') });
      await within(dialog).findByText(i18n.t('usage.rates.catalogue.none'), { exact: true });
      expect(within(dialog).getAllByText(i18n.t('usage.rates.catalogue.none'), { exact: true })).toHaveLength(1);
      expect(dialog).toHaveTextContent(i18n.t('usage.rates.catalogue.noneAbout'));
      expect(within(dialog).queryByRole('alert')).not.toBeInTheDocument();
      expect(within(dialog).queryByRole('textbox')).not.toBeInTheDocument();
      expect(fetch).toHaveBeenCalledTimes(1);
    } finally {
      view.unmount();
      await setLocale(previous);
      if (oldTheme === null) document.documentElement.removeAttribute('data-theme');
      else document.documentElement.setAttribute('data-theme', oldTheme);
    }
  },
);

it('keeps an unmatched model distinct from a catalogue that was never loaded', async () => {
  vi.stubGlobal('fetch', vi.fn((request: Request) => Promise.resolve(Response.json(
    new URL(request.url).searchParams.has('q') ? { ...loaded, families: [] } : loaded,
  ))));
  show(<CatalogueDialog open onOpenChange={() => undefined} />);
  await screen.findByRole('table', { name: i18n.t('usage.rates.catalogue.table') });
  const user = userEvent.setup();
  await user.type(screen.getByRole('textbox', { name: i18n.t('usage.rates.catalogue.name') }), 'missing-model');
  await user.click(screen.getByRole('button', { name: i18n.t('usage.rates.catalogue.look') }));
  expect(await screen.findByRole('status')).toHaveTextContent(i18n.t('usage.rates.catalogue.matchesNone', { name: 'missing-model' }));
  expect(screen.getByRole('table', { name: i18n.t('usage.rates.catalogue.table') })).toHaveTextContent('example-*');
  expect(screen.queryByText(i18n.t('usage.rates.catalogue.none'), { exact: true })).not.toBeInTheDocument();
});

it('does not turn a real 404 into an empty catalogue', async () => {
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(Response.json({
    type: 'orch:error:not_found', code: 'not_found', title: 'Not found', status: 404,
    detail: 'The route is missing.', request_id: 'catalogue-request',
  }, { status: 404 }))));
  show(<CatalogueDialog open onOpenChange={() => undefined} />);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByText(i18n.t('usage.rates.catalogue.none'), { exact: true })).not.toBeInTheDocument();
});
