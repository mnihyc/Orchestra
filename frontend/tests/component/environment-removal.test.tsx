/** Deletion explains the declared lifecycle, keeps borrowed directories and retries one acceptance. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { RemoveEnvironmentDialog } from '@/features/resources/environments/Remove';
import type { Environment, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({ remove: vi.fn<(id: string, name: string, key: string, review: { expected_keep_days?: number; expected_revision?: number }) => Promise<Environment>>(), details: vi.fn<() => Promise<Environment>>(), settings: vi.fn<() => Promise<ResolvedSetting[]>>(), close: vi.fn(), back: vi.fn() }));
vi.mock('@/api/trace', async (load) => ({ ...await load<object>(), scheduleEnvironmentRemoval: server.remove }));
vi.mock('@/api/environments', async (load) => ({ ...await load<object>(), environmentQuery: (id: string) => ({ queryKey: ['delete-current', id], queryFn: server.details }), environmentSettingsQuery: (id: string) => ({ queryKey: ['delete-policy', id], queryFn: server.settings }) }));
const unit: Environment = { ...detailEnvironment, name: 'Docker workspace', facts: { environment_descriptor: { lifecycle: 'unit' } } };
const beforeAppearance = useAppearance.getState();
beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks();
  server.details.mockResolvedValue(unit);
  server.remove.mockReset().mockResolvedValue({ ...unit, status: 'removed' });
  server.settings.mockResolvedValue([{ key: 'env.removed_keep_days', value: 7, source: { scope: 'default', node: null }, own: null, writable: true, locked_by: null, parts: [] }]);
});
afterEach(async () => { cleanup(); useAppearance.setState(beforeAppearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`a managed unit with a machine names the unit and retained volumes (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const user = userEvent.setup();
    show(<RemoveEnvironmentDialog environment={unit} onClose={server.close} afterDelete={server.back} />);
    expect(unit.machine).not.toBeNull();
    expect(await screen.findByText(i18n.t('trace.removalUnit'))).toBeVisible();
    expect(screen.queryByText(i18n.t('trace.removalConnected'))).not.toBeInTheDocument();
    await waitFor(() => expect(server.settings).toHaveBeenCalledOnce());
    expect(screen.getByText(i18n.t('environmentData.purge'))).toBeVisible();
    await user.click(screen.getByRole('textbox', { name: i18n.t('environments.remove.type', { name: unit.name }) }));
    await user.paste(unit.name);
    await user.click(screen.getByRole('button', { name: i18n.t('trace.removeConfirm', { name: unit.name }) }));
    await waitFor(() => expect(server.remove).toHaveBeenCalledOnce());
    expect(server.remove.mock.calls[0]?.slice(0, 2)).toEqual([unit.id, unit.name]);
    expect(server.close).toHaveBeenCalledOnce(); expect(server.back).toHaveBeenCalledOnce();
  });

  it(`a registered directory keeps its files regardless of display placement (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const directory: Environment = { ...unit, template: null, host: null, facts: { environment_descriptor: { lifecycle: 'attach' } } };
    server.details.mockResolvedValue(directory);
    show(<RemoveEnvironmentDialog environment={directory} onClose={server.close} />);
    expect(await screen.findByText(i18n.t('trace.removalConnected'))).toBeVisible();
    expect(screen.queryByText(i18n.t('trace.removalUnit'))).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('environmentData.purge'))).not.toBeInTheDocument();
    expect(server.settings).not.toHaveBeenCalled();
  });
}

it('temporary consent produces the immediate owned-volume warning even if the ordinary policy keeps seven days', async () => {
  const temporary = { ...unit, config: { 'env.lifetime': 'temporary' }, removal_keep_days: 0 };
  server.details.mockResolvedValue(temporary);
  show(<RemoveEnvironmentDialog environment={temporary} onClose={server.close} />);
  expect(await screen.findByText(i18n.t('trace.removalUnitZero'))).toBeVisible();
  expect(screen.queryByText(i18n.t('trace.removalUnit'))).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('uses the authoritative bounded window and names borrowed and owned roots independently (%s)', async (locale) => {
  await setLocale(locale); useAppearance.setState({ locale, timeZone: 'UTC' });
  const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.UTC(2026, 3, 1, 12));
  const environment = { ...unit, removal_keep_days: 1, data_sources: { workspace: { kind: 'existing_directory' as const, path: '/external/work' }, home: { kind: 'made_by_orch' as const } } };
  server.details.mockResolvedValue(environment);
  server.remove.mockResolvedValue({ ...environment, status: 'removed' });
  try {
    show(<RemoveEnvironmentDialog environment={environment} onClose={server.close} />);
    expect(await screen.findByText(locale === 'en' ? 'Left in place' : '原地保留')).toBeVisible();
    expect(screen.getByText(locale === 'en' ? 'Kept with owned data' : '与自有数据一同保留')).toBeVisible();
    expect(screen.getByRole('dialog')).toHaveTextContent(locale === 'en' ? 'Apr 2, 2026' : '2026年4月2日');
    const user = userEvent.setup();
    await user.type(screen.getByRole('textbox', { name: i18n.t('environments.remove.type', { name: environment.name }) }), environment.name);
    const confirm = screen.getByRole('button', { name: i18n.t('trace.removeConfirm', { name: environment.name }) });
    await waitFor(() => expect(confirm).not.toHaveAttribute('aria-disabled', 'true'));
    await user.click(confirm);
    expect(server.remove).toHaveBeenCalledWith(environment.id, environment.name, expect.any(String), { expected_keep_days: 1, expected_revision: environment.revision });
  } finally { clock.mockRestore(); }
});

it('unknown lifecycle facts explain why deletion cannot be confirmed', async () => {
  const user = userEvent.setup();
  const unknown = { ...unit, facts: {}, template: null };
  server.details.mockResolvedValue(unknown);
  show(<RemoveEnvironmentDialog environment={unknown} onClose={server.close} />);
  expect(await screen.findByText(i18n.t('trace.removalKindUnknown'))).toBeVisible();
  await user.click(screen.getByRole('textbox', { name: i18n.t('environments.remove.type', { name: unit.name }) }));
  await user.paste(unit.name);
  const confirm = screen.getByRole('button', { name: i18n.t('trace.removeConfirm', { name: unit.name }) });
  expect(confirm).toHaveAttribute('aria-disabled', 'true'); await user.click(confirm);
  expect(server.remove).not.toHaveBeenCalled();
});

it('a lost answer keeps the confirmation and retries with the same idempotency identity', async () => {
  const user = userEvent.setup(); server.remove.mockRejectedValueOnce(new Error('lost response'));
  show(<RemoveEnvironmentDialog environment={unit} onClose={server.close} />);
  const name = screen.getByRole('textbox', { name: i18n.t('environments.remove.type', { name: unit.name }) });
  await user.click(name); await user.paste(unit.name);
  const confirm = screen.getByRole('button', { name: i18n.t('trace.removeConfirm', { name: unit.name }) });
  await user.click(confirm); await waitFor(() => expect(server.remove).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(confirm).not.toHaveAttribute('aria-busy', 'true'));
  expect(name).toHaveValue(unit.name); expect(server.close).not.toHaveBeenCalled();
  await user.click(confirm); await waitFor(() => expect(server.remove).toHaveBeenCalledTimes(2));
  const first = server.remove.mock.calls[0], second = server.remove.mock.calls[1];
  if (first === undefined || second === undefined) throw new Error('both delete attempts must be captured');
  expect(first[2]).not.toBe(''); expect(second).toEqual(first);
});
