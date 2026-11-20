/** Every member's kept-resource rows retain their own accessible heading in a batch. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as EnvironmentsApi from '@/api/environments';
import type * as ResourcesApi from '@/api/resources';
import type * as ForksApi from '@/api/forks';
import { BatchChange } from '@/features/change/BatchChange';
import type { ForkBatchPlan, Provider, ProviderModel } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { detailEnvironment } from './resource-detail-fixtures';
import { resourcePlan } from './plan-resource-fixtures';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show, skipOverflowFallback } from './support';
import { deferred } from './deferred';
// These cases prove choices and retained rows; text measurement needs browser layout.
skipOverflowFallback();
const io = vi.hoisted(() => ({ plan: vi.fn<typeof ForksApi.planBatch>(), providers: [] as Provider[], models: [] as ProviderModel[], readModels: vi.fn<() => Promise<ProviderModel[]>>() }));
vi.mock('@/api/environments', async (load) => ({ ...await load<typeof EnvironmentsApi>(), environmentsQuery: () => ({ queryKey: ['kept-batch-environments'], queryFn: () => Promise.resolve([{ ...detailEnvironment, id: 'destination', name: 'Destination', access: { ...detailEnvironment.access, permissions: ['environment.manage'] } }]) }) }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), providersQuery: () => ({ queryKey: ['kept-batch-providers'], queryFn: () => Promise.resolve(io.providers) }), agentsQuery: () => ({ queryKey: ['kept-batch-agents'], queryFn: () => Promise.resolve([]) }), providerModelsQuery: (id: string) => ({ queryKey: ['kept-batch-models', id], queryFn: io.readModels }) }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof ForksApi>(), planBatch: io.plan }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
beforeEach(() => {
  io.providers = []; io.models = [];
  io.readModels.mockReset().mockImplementation(() => Promise.resolve(io.models));
  io.plan.mockReset(); const first = resourcePlan('First');
  io.plan.mockResolvedValue({ preflight_id: 'batch-plan', expires_at: first.expires_at, executable: 2, options: first.options, estimate: first.estimate, plans: [first, resourcePlan('Second')], skipped: [], turns_running: 0, to: { provider: null, agent: null, model: null, effort: null } });
});

it.each(['en', 'zh-CN'] as const)('finds the model and its reported effort for a batch without substituting another choice in %s', async (locale) => {
  await setLocale(locale);
  io.providers = [planProvider()];
  io.models = Array.from({ length: 12 }, (_, at) => ({ ...MODEL_ESTIMATE, id: `model-${at}`, model_name: `model-${at}`, display_name: `Batch model ${at}`,
    enabled: true, source: 'manual', groups: [], parameters: { efforts: Array.from({ length: 12 }, (_, level) => `level-${level}`) } }));
  const models = deferred<ProviderModel[]>();
  io.readModels.mockReturnValue(models.promise);
  // Choosing a target stays available while an earlier plan is still being checked.
  const checking = deferred<ForkBatchPlan>();
  io.plan.mockReturnValue(checking.promise);
  show(<BatchChange source={{ kind: 'threads', threads: [{ id: 'First', name: 'First' }], label: 'Selected' }} batch={null} open onClose={vi.fn()} />);
  const provider = await screen.findByRole('combobox', { name: i18n.t('change.runs.provider') });
  act(() => provider.focus());
  fireEvent.click(provider);
  fireEvent.click(await screen.findByRole('option', { name: 'Team Pro' }));
  const picker = await screen.findByRole('combobox', { name: i18n.t('change.runs.model') });
  await waitFor(() => expect(picker).toBeEnabled());
  act(() => picker.focus());
  fireEvent.click(picker);
  expect(picker).toHaveFocus();
  fireEvent.change(picker, { target: { value: 'Batch model 11' } });
  expect(picker).toHaveValue('Batch model 11');
  expect(screen.queryByRole('option', { name: 'Batch model 11' })).not.toBeInTheDocument();
  models.resolve(io.models);
  const reported = await screen.findByRole('option', { name: 'Batch model 11' });
  expect(reported).toBeVisible();
  expect(reported).not.toHaveAttribute('aria-disabled', 'true');
  expect(screen.queryByRole('option', { name: 'Batch model 10' })).not.toBeInTheDocument();
  fireEvent.click(reported);
  const effort = await screen.findByRole('combobox', { name: i18n.t('change.runs.effort') });
  act(() => effort.focus());
  fireEvent.click(effort);
  expect(effort).toHaveFocus();
  fireEvent.change(effort, { target: { value: 'level-11' } });
  expect(effort).toHaveValue('level-11');
  fireEvent.click(await screen.findByRole('option', { name: 'level-11' }));
  await waitFor(() => expect(io.plan.mock.calls.some(([body]) => body.to?.model === 'model-11' && body.to.effort === 'level-11')).toBe(true));
});
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it('shows both members’ retained resources before confirmation in '+locale, async () => {
  await setLocale(locale); const user = userEvent.setup(); show(<BatchChange source={{ kind: 'threads', threads: [{ id: 'First', name: 'First' }, { id: 'Second', name: 'Second' }], label: 'Selected' }} batch={null} open onClose={vi.fn()} />);
  await user.click(await screen.findByRole('combobox', { name: i18n.t('loss.destination') }));
  await user.click(await screen.findByRole('option', { name: 'Destination' }));
  const sections = await screen.findAllByRole('region', { name: i18n.t('change.lists.stays') });
  expect(sections).toHaveLength(2);
  expect(new Set(sections.map((section) => section.getAttribute('aria-labelledby'))).size).toBe(2);
  for (const section of sections) {
    expect(section).toHaveTextContent(i18n.t('mcpPlan.kept', { name: 'Research' }));
    expect(section).toHaveTextContent('Shared data'); expect(within(section).getAllByRole('listitem')).toHaveLength(2);
  }
  expect(io.plan).toHaveBeenCalledOnce();
});
