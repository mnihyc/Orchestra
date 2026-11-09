/** Explicit opt-in, exact credit strings and retained provider options use the real edit dialog. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ProviderServices } from '@/features/resources/providers/Services';
import type { Provider } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const { update } = vi.hoisted(() => ({ update: vi.fn() }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof import('@/api/resources')>(), updateProvider: update }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

function provider(): Provider {
  return { ...planProvider({ kind: 'api' }), service_offerings: {
    'fixture-search': { enabled: false, prices: {}, future_option: { kept: true } },
    other: { enabled: true, groups: ['other'], prices: {} },
  }, service_catalogue: [{ id: 'fixture-search', title: 'Recorded Search', capabilities: ['web_search'], enabled: false, available: true,
    groups: ['web'], declared_groups: ['web'], units: ['search'], catalogue_prices: { search: { amount: '0.005', currency: 'USD', unit: 'search' } },
    prices: { search: { amount: '0.005', currency: 'USD', unit: 'search' } }, explicit_prices: [] }] };
}

beforeEach(() => { update.mockResolvedValue({ projection: 'receipt', provider_id: 'plan-provider' }); });
afterEach(async () => { update.mockReset(); await setLocale('en'); });

// EN: Provider → Services → Edit → enable and save; the browser path is provider-services.spec.ts.
// 中文：提供方 → 服务 → 编辑 → 启用并保存；浏览器路径见 provider-services.spec.ts。
for (const locale of ['en', 'zh-CN'] as const) it(`saves only an explicit service edit and retains exact money and other choices (${locale})`, async () => {
  await setLocale(locale);
  const person = userEvent.setup();
  const current = provider();
  show(<ProviderServices provider={current} />);
  expect(update).not.toHaveBeenCalled();
  await person.click(screen.getByRole('button', { name: i18n.t('providerServices.edit') }));
  const dialog = within(screen.getByRole('dialog', { name: i18n.t('providerServices.edit') }));
  await person.click(dialog.getByRole('checkbox', { name: i18n.t('providerServices.enable') }));
  await person.click(dialog.getByRole('checkbox', { name: i18n.t('providerServices.inheritGroups') }));
  const groups = dialog.getByRole('textbox', { name: i18n.t('providerServices.groups') });
  await person.clear(groups);
  await person.type(groups, 'web, research');
  await person.click(dialog.getByRole('combobox', { name: i18n.t('providerServices.price', { unit: i18n.t('providerServices.units.search') }) }));
  await person.click(screen.getByRole('option', { name: i18n.t('providerServices.creditsPrice') }));
  await person.type(dialog.getByRole('textbox', { name: i18n.t('providerServices.credits', { unit: i18n.t('providerServices.units.search') }) }), '0.000750');
  await person.click(dialog.getByRole('button', { name: i18n.t('resources.edit.save') }));
  expect(update).toHaveBeenCalledTimes(1);
  expect(update).toHaveBeenCalledWith(current.id, { expected_revision: current.revision, service_offerings: {
    'fixture-search': { enabled: true, groups: ['web', 'research'], prices: { search: { amount: '0.000750', currency: 'credits', unit: 'search' } }, future_option: { kept: true } },
    other: current.service_offerings?.other,
  } });
});

it('preserves a saved native-currency quote instead of pretending to convert it', async () => {
  const person = userEvent.setup();
  const current = provider();
  current.service_offerings = { 'fixture-search': { enabled: false, prices: { search: { amount: '0.012345', currency: 'USD', unit: 'search', future_quote: true } } } };
  show(<ProviderServices provider={current} />);
  await person.click(screen.getByRole('button', { name: i18n.t('providerServices.edit') }));
  const dialog = within(screen.getByRole('dialog', { name: i18n.t('providerServices.edit') }));
  expect(dialog.getByRole('combobox')).toHaveTextContent(i18n.t('providerServices.savedPrice', { currency: 'USD' }));
  await person.click(dialog.getByRole('checkbox', { name: i18n.t('providerServices.enable') }));
  await person.click(dialog.getByRole('button', { name: i18n.t('resources.edit.save') }));
  expect(update.mock.calls[0]?.[1]?.service_offerings?.['fixture-search']?.prices).toEqual(current.service_offerings['fixture-search']?.prices);
});

it('distinguishes unreadable catalogue data from a known empty catalogue', () => {
  const hidden = provider();
  hidden.service_catalogue = null;
  const drawn = show(<ProviderServices provider={hidden} />);
  expect(screen.queryByText(i18n.t('providerServices.empty'))).not.toBeInTheDocument();
  drawn.rerender(<ProviderServices provider={{ ...hidden, service_catalogue: [] }} />);
  expect(screen.getByText(i18n.t('providerServices.empty'))).toBeVisible();
  expect(update).not.toHaveBeenCalled();
});

it('keeps unavailable enablement disabled and a refused save leaves the draft intact', async () => {
  const person = userEvent.setup();
  const current = provider();
  current.service_catalogue = current.service_catalogue?.map((offering) => ({ ...offering, available: false })) ?? [];
  show(<ProviderServices provider={current} />);
  await person.click(screen.getByRole('button', { name: i18n.t('providerServices.edit') }));
  const dialog = within(screen.getByRole('dialog', { name: i18n.t('providerServices.edit') }));
  expect(dialog.getByRole('checkbox', { name: i18n.t('providerServices.enable') })).toBeDisabled();
  expect(dialog.getByText(i18n.t('providerServices.unavailable'))).toBeVisible();
  await person.click(dialog.getByRole('checkbox', { name: i18n.t('providerServices.inheritGroups') }));
  const groups = dialog.getByRole('textbox', { name: i18n.t('providerServices.groups') });
  await person.clear(groups);
  await person.type(groups, 'research');
  update.mockRejectedValueOnce(new Error('The reviewed provider changed'));
  await person.click(dialog.getByRole('button', { name: i18n.t('resources.edit.save') }));
  expect(await dialog.findByRole('alert')).toBeVisible();
  expect(groups).toHaveValue('research');
});
