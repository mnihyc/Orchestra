/** Every member's kept-resource rows retain their own accessible heading in a batch. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { BatchChange } from '@/features/change/BatchChange';
import type { ForkBatchPlan } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { detailEnvironment } from './resource-detail-fixtures';
import { resourcePlan } from './plan-resource-fixtures';
import { show } from './support';
const io = vi.hoisted(() => ({ plan: vi.fn<() => Promise<ForkBatchPlan>>() }));
vi.mock('@/api/environments', async (load) => ({ ...await load<typeof import('@/api/environments')>(), environmentsQuery: () => ({ queryKey: ['kept-batch-environments'], queryFn: () => Promise.resolve([{ ...detailEnvironment, id: 'destination', name: 'Destination', access: { ...detailEnvironment.access, permissions: ['environment.manage'] } }]) }) }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof import('@/api/resources')>(), providersQuery: () => ({ queryKey: ['kept-batch-providers'], queryFn: () => Promise.resolve([]) }), agentsQuery: () => ({ queryKey: ['kept-batch-agents'], queryFn: () => Promise.resolve([]) }) }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof import('@/api/forks')>(), planBatch: io.plan }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
beforeEach(() => {
  io.plan.mockReset(); const first = resourcePlan('First');
  io.plan.mockResolvedValue({ preflight_id: 'batch-plan', expires_at: first.expires_at, executable: 2, options: first.options, estimate: first.estimate, plans: [first, resourcePlan('Second')], skipped: [], turns_running: 0, to: { provider: null, agent: null, model: null, effort: null } });
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
