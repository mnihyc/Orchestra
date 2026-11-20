/** Catalogue facts stay readable on phones; editing binds the reviewed revision. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { GatewayCatalogueView } from '@/features/gateways/GatewayCatalogue';
import { i18n } from '@/i18n';
import { show } from '../support';
import { catalogue, look, modes, restore } from './fixtures';

afterEach(restore);
it.each(modes)('shows labels, regions and aliases, and keeps the revision it binds to out of sight, in $locale/$theme at $width', async (mode) => {
  await look(mode);
  const save = vi.fn().mockResolvedValue(undefined);
  const view = show(<GatewayCatalogueView catalogue={catalogue} writable onSave={save} />);
  for (const gateway of catalogue.gateways) {
    expect(screen.getByText(gateway.label, { exact: true })).toBeVisible();
    expect(screen.getByText(gateway.region, { exact: true })).toBeVisible();
    for (const endpoint of gateway.endpoints) expect(screen.getByText(endpoint.url, { exact: true })).toBeVisible();
  }
  expect(view.container).not.toHaveTextContent(catalogue.revision);
  expect(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('regionalGateways.title') }) }))
    .toHaveAttribute('data-help-heading', 'gateway-catalogue');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.edit') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('regionalGateways.editTitle') });
  const first = within(dialog).getByRole('group', { name: i18n.t('regionalGateways.gatewayNumber', { number: 1 }) });
  const edited = within(dialog).getByRole('list', { name: i18n.t('regionalGateways.catalogue') });
  expect(edited.querySelectorAll(':scope > li')).toHaveLength(catalogue.gateways.length);
  expect(edited.querySelector(':scope > li')).toContainElement(first);
  const endpoints = within(first).getByRole('list', { name: i18n.t('regionalGateways.endpointsFor', { label: 'Europe gateway' }) });
  expect(within(endpoints).getAllByRole('listitem')).toHaveLength(catalogue.gateways[0]!.endpoints.length);
  expect(within(first).getByRole('textbox', { name: i18n.t('regionalGateways.label') })).toHaveValue('Europe gateway');
  expect(within(first).getByRole('textbox', { name: i18n.t('regionalGateways.endpointNumber', { number: 1 }) })).toHaveValue(catalogue.gateways[0]!.endpoints[0]!.url);
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('common.cancel') }));
  expect(save).not.toHaveBeenCalled();
});

it('sends one ordered replacement with the original reviewed revision and stable existing IDs', async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  const view = show(<GatewayCatalogueView catalogue={catalogue} writable onSave={save} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.edit') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('regionalGateways.editTitle') });
  const name = within(dialog).getAllByRole('textbox', { name: i18n.t('regionalGateways.label') })[0]!;
  expect(within(dialog).getAllByRole('textbox', { name: i18n.t('regionalGateways.id') })[0]).toHaveAttribute('readonly');
  await userEvent.clear(name); await userEvent.type(name, 'Europe renamed');
  view.rerender(<GatewayCatalogueView catalogue={{ ...catalogue, revision: 'changed-elsewhere' }} writable onSave={save} />);
  expect(save).not.toHaveBeenCalled();
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(save).toHaveBeenCalledExactlyOnceWith({ expected_revision: catalogue.revision,
    gateways: [{ ...catalogue.gateways[0], label: 'Europe renamed' }, catalogue.gateways[1]] }));
});

it('keeps a refused edit local and does not retry or replace its revision', async () => {
  const save = vi.fn().mockRejectedValue(new Error('The catalogue changed.'));
  show(<GatewayCatalogueView catalogue={catalogue} writable onSave={save} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.edit') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('regionalGateways.editTitle') });
  const name = within(dialog).getAllByRole('textbox', { name: i18n.t('regionalGateways.label') })[0]!;
  await userEvent.clear(name); await userEvent.type(name, 'Retained draft');
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(name).toHaveValue('Retained draft');
  expect(dialog).toBeVisible();
});

it('does not enable catalogue editing for a read-only reader', () => {
  show(<GatewayCatalogueView catalogue={catalogue} onSave={vi.fn()} />);
  expect(screen.getByRole('button', { name: i18n.t('regionalGateways.edit') })).toBeDisabled();
});

it('reorders logical gateways without changing identities, aliases or the reviewed revision', async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  show(<GatewayCatalogueView catalogue={catalogue} writable onSave={save} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.edit') }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.moveUp', { label: 'Asia tools' }) }));
  expect(save).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(save).toHaveBeenCalledExactlyOnceWith({ expected_revision: catalogue.revision,
    gateways: [catalogue.gateways[1], catalogue.gateways[0]] }));
});
