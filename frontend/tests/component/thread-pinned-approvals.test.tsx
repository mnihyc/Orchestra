/** A waiting decision has one native form above the composer in every detail mode. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Approvals from '@/api/approvals';
import type * as Live from '@/realtime/live';
import type * as Personal from '@/api/personal';
import { PinnedApprovals } from '@/features/thread/PinnedApprovals';
import { ApprovalCard } from '@/features/timeline/items/Approval';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import type { Approval, Item } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ALLOW_DENY, approvalItem, harnessApproval } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const wire = vi.hoisted(() => ({ read: vi.fn<() => Promise<Approval>>(), decide: vi.fn<typeof Approvals.decideApproval>() }));
const ADA = userRef(me(1, 'ada', 'Ada'));
vi.mock('@/api/approvals', async (original) => {
  const actual = await original<typeof Approvals>();
  return { ...actual, approvalQuery: (id: string) => ({ ...actual.approvalQuery(id), queryFn: wire.read }), decideApproval: wire.decide };
});
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(), meStateQuery: { queryKey: ['pinned-state'], queryFn: () => Promise.resolve({}) } }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
const ids: string[] = [];
afterEach(async () => { cleanup(); for (const id of ids.splice(0)) threadWriter.evict(id); vi.resetAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const detail of ['summary', 'steps', 'everything'] as const) {
  it(`pins one exact decision and retires it after its confirmed answer in ${locale}/${detail}`, async () => {
    await setLocale(locale); const source = snapshot(null); const id = source.thread.id; ids.push(id);
    const request = harnessApproval('pinned-command', { threadId: id, itemId: 'pinned-item', requester: ADA, title: 'Inspect output',
      request: { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'ls', cwd: '/work', reason: 'Check the generated files' } } });
    const item: Item = { ...approvalItem('pinned-item', request.id), kind: 'approval', payload: { approval_id: request.id, approval: request } };
    threadWriter.reset(id, { ...source, items: [item] }, false);
    wire.decide.mockResolvedValue({ ...request, state: 'approved', decided_by: ADA, decision: { option_id: 'allow_once' } });
    const context: TimelineContext = { threadId: id, threadName: source.thread.name, detail, pinApprovals: true,
      recent: new Set(), layout: 'one', setLayout: () => undefined, meId: ADA.id, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
    const view = show(<div data-screen="thread"><TimelineContextValue.Provider value={context}><ApprovalCard item={item} /></TimelineContextValue.Provider><PinnedApprovals threadId={id} /></div>);
    const pinned = screen.getByRole('region', { name: i18n.t('thread.pinnedApprovals') });
    expect(within(pinned).getByText('Check the generated files')).toBeVisible();
    const once = `${i18n.t('approvals.allowOnce')} · ls`;
    expect(screen.getAllByRole('button', { name: once })).toHaveLength(1);
    expect(within(pinned).getByRole('button', { name: `${i18n.t('approvals.allowSession')} · ls` })).toBeVisible();
    expect(within(pinned).getByRole('button', { name: `${i18n.t('approvals.deny')} · ls` })).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: once }));
    await waitFor(() => expect(wire.decide).toHaveBeenCalledExactlyOnceWith(request.id, { option_id: 'allow_once' }));
    await waitFor(() => expect(view.container.querySelector('[data-pinned-approvals]')).toBeNull());
    expect(screen.queryByRole('button', { name: once })).not.toBeInTheDocument();
    expect(wire.read).not.toHaveBeenCalled();
  });
}
