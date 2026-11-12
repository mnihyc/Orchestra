/** Only the connector's typed metadata is rendered; sealed entries expose their safe name and size alone. */
import { cleanup, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { MachineStorages } from '@/features/resources/machines/Storages';
import { MachineDataDirectory } from '@/features/resources/machines/DataDirectory';
import type { MachineDataInventory } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { formatBytes } from '@/lib/format';
import { show } from './support';
const observed = '2026-10-15T08:12:00Z';
const inventory = (patch: Partial<MachineDataInventory> = {}): MachineDataInventory => ({ observed_at: observed, complete: true, unlisted_parts: 0, problem: null, parts: [
  { id: 'environments-id', kind: 'environment_data', name: 'environments', bytes: 1024, problem: null },
  { id: 'logs-id', kind: 'logs', name: 'logs', bytes: 0, problem: null },
  { id: 'sealed-private-id', kind: 'sealed_store', name: 'vault.bin', bytes: 128, problem: null },
], ...patch });
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows native parts and the observation age beside existing capacity (${locale})`, async () => {
    await setLocale(locale); show(<MachineStorages facts={{ storages: [{ name: 'fast', default: true, capacity_gb: 100, free_gb: 40 }] }} dataDirectory={inventory()} />);
    expect(screen.getByText(i18n.t('machineStorage.default', { name: 'fast' }))).toBeInTheDocument();
    const parts = await screen.findByRole('table', { name: i18n.t('machineData.parts') }); expect(within(parts).getByText('environments')).toBeInTheDocument();
    expect(within(parts).getByText(formatBytes(1024, locale))).toBeInTheDocument(); expect(within(parts).getByText(formatBytes(0, locale))).toBeInTheDocument();
    expect(document.querySelector(`time[datetime="${observed}"]`)).not.toBeNull(); expect(screen.getByText(i18n.t('machineData.observed'))).toBeInTheDocument();
  });
  it(`distinguishes incomplete and omitted components from zero size (${locale})`, async () => {
    await setLocale(locale); show(<MachineDataDirectory inventory={inventory({ complete: false, unlisted_parts: 3, problem: 'unreadable', parts: [{ id: 'link-id', kind: 'other', name: 'linked-part', bytes: null, problem: 'link' }] })} />);
    expect(screen.getByText(new RegExp(i18n.t('machineData.incomplete')))).toBeInTheDocument(); expect(screen.getByText(/3/)).toHaveTextContent(i18n.t('machineData.unlisted', { number: '3' }));
    expect(screen.getByText(i18n.t('machineData.problems.link'))).toBeInTheDocument(); expect(screen.getByText('–')).toBeInTheDocument(); expect(screen.queryByText(formatBytes(0, locale))).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('machineData.empty'))).not.toBeInTheDocument();
  });
  it(`a sealed store has only name and size even when inspection was incomplete (${locale})`, async () => {
    await setLocale(locale); show(<MachineDataDirectory inventory={inventory({ complete: false, parts: [{ id: 'not-a-remote-file-id', kind: 'sealed_store', name: 'credentials.db', bytes: null, problem: 'unreadable' }] })} />);
    const table = screen.getByRole('table', { name: i18n.t('machineData.sealed') }); expect(within(table).getAllByRole('columnheader')).toHaveLength(2);
    expect(within(table).getByText('credentials.db')).toBeInTheDocument(); expect(within(table).getByText('–')).toBeInTheDocument();
    expect(within(table).queryByText(i18n.t('machineData.problems.unreadable'))).not.toBeInTheDocument(); expect(table).not.toHaveTextContent('not-a-remote-file-id');
    expect(within(table).queryByRole('link')).not.toBeInTheDocument(); expect(within(table).queryByRole('button')).not.toBeInTheDocument();
  });
  it(`unavailable, fully empty and a failed root scan remain distinct (${locale})`, async () => {
    await setLocale(locale); const view = show(<MachineDataDirectory />); expect(screen.getByText(i18n.t('machineData.unavailable'))).toBeInTheDocument();
    view.rerender(<MachineDataDirectory inventory={inventory({ parts: [] })} />); expect(screen.getByText(i18n.t('machineData.empty'))).toBeInTheDocument();
    view.rerender(<MachineDataDirectory inventory={inventory({ parts: [], complete: false, problem: 'size_overflow' })} />);
    expect(screen.getByText(new RegExp(i18n.t('machineData.problems.size_overflow')))).toBeInTheDocument(); expect(screen.queryByText(i18n.t('machineData.empty'))).not.toBeInTheDocument();
  });
}
