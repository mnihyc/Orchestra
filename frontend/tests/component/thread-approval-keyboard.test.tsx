/** Waiting native answers retain their scope and exact request under pointer and keyboard interaction. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Approvals from '@/api/approvals';
import type * as Live from '@/realtime/live';
import { PinnedApprovals } from '@/features/thread/PinnedApprovals';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import type { Approval, HarnessRequest, Item } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { threadWriter } from '@/realtime/stores/thread';
import { ALLOW_DENY, approvalItem, change, commandThreadOptions, harnessApproval } from '../doubles/thread-rows';
import { snapshot } from '../unit/continuation-fixtures';
import { deferred } from './deferred';
import { show } from './support';

const wire = vi.hoisted(() => ({ decide: vi.fn<typeof Approvals.decideApproval>() }));
vi.mock('@/api/approvals', async (original) => ({ ...await original<typeof Approvals>(), decideApproval: wire.decide }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
const ids: string[] = []; const appearance = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme; const darkBefore = document.documentElement.classList.contains('dark');
afterEach(async () => { cleanup(); for (const id of ids.splice(0)) threadWriter.evict(id); wire.decide.mockReset(); useAppearance.setState(appearance); document.documentElement.classList.toggle('dark', darkBefore); if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore; await setLocale('en'); });
const COMMAND = 'rg redirect src'; const CWD = '/work'; const REASON = 'Check the redirect before changing it';
function draw(request: HarnessRequest, width = 1440) {
  const source = snapshot(null); const id = source.thread.id; ids.push(id);
  const approval = harnessApproval('keyboard-request', { threadId: id, itemId: 'keyboard-item', requester: source.thread.owner, title: request.request_kind === 'command' ? COMMAND : 'Changes to src/login.ts', request });
  const item: Item = { ...approvalItem('keyboard-item', approval.id), thread_id: id, kind: 'approval', payload: { approval_id: approval.id, approval } };
  threadWriter.reset(id, { ...source, items: [item] }, false);
  return { ...show(<ViewCompactContext.Provider value={width === 400}><div style={{ width }}><PinnedApprovals threadId={id} /></div></ViewCompactContext.Provider>), approval };
}
const commandRequest = (): HarnessRequest => ({ request_kind: 'command', options: commandThreadOptions(COMMAND, CWD), detail: { command: COMMAND, cwd: CWD, reason: REASON } });
const answerName = (choice: 'allowOnce' | 'allowThread' | 'deny', target = COMMAND) => `${i18n.t(`approvals.${choice}`)} · ${target}`;

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`offers the three native answers, keeps Escape harmless and sends focused Enter once at ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const pending = deferred<Approval>(); wire.decide.mockReturnValue(pending.promise);
    const { approval, container } = draw(commandRequest(), width);
    const card = screen.getByRole('region', { name: COMMAND });
    expect(within(card).getByText(REASON)).toBeVisible(); expect(within(card).getByText(i18n.t('approvals.in', { directory: CWD }))).toBeVisible();
    expect(container.querySelector('[data-approval-rule]')).toHaveTextContent(i18n.t('approvals.matcher.exact', { command: COMMAND, cwd: CWD }));
    const names = ['allowOnce', 'allowThread', 'deny'] as const;
    expect(within(card).getAllByRole('button').filter((button) => button.dataset.action === 'allow' || button.dataset.action === 'deny').map((button) => button.getAttribute('aria-label'))).toEqual(names.map((choice) => answerName(choice)));
    expect(within(card).queryByRole('button', { name: `${i18n.t('approvals.allowSession')} · ${COMMAND}` })).toBeNull();
    for (const choice of names) expect(document.activeElement).not.toBe(within(card).getByRole('button', { name: answerName(choice) }));
    const user = userEvent.setup(); act(() => within(card).getByRole('button', { name: answerName('deny') }).focus());
    await user.keyboard('{Escape}'); expect(wire.decide).not.toHaveBeenCalled();
    act(() => within(card).getByRole('button', { name: answerName('allowThread') }).focus());
    await user.keyboard('{Enter}'); await waitFor(() => expect(wire.decide).toHaveBeenCalledExactlyOnceWith(approval.id, { option_id: 'allow_thread' }));
    await user.keyboard('{Enter}{Escape}'); expect(wire.decide).toHaveBeenCalledTimes(1);
    expect(within(card).getByRole('button', { name: answerName('deny') })).toBeDisabled();
    await act(async () => { pending.resolve({ ...approval, state: 'approved', decision: { option_id: 'allow_thread' } }); await pending.promise; });
    await waitFor(() => expect(container.querySelector('[data-pinned-approvals]')).toBeNull());
  });
}
for (const locale of ['en', 'zh-CN'] as const) for (const [choice, optionId] of [['allowOnce', 'allow_once'], ['deny', 'deny']] as const) {
  it(`answers only the focused ${choice} option with Enter in ${locale}`, async () => {
    await setLocale(locale); const { approval } = draw(commandRequest());
    wire.decide.mockResolvedValue({ ...approval, state: choice === 'deny' ? 'denied' : 'approved', decision: { option_id: optionId } });
    act(() => screen.getByRole('button', { name: answerName(choice) }).focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(wire.decide).toHaveBeenCalledExactlyOnceWith(approval.id, { option_id: optionId }));
  });
}
for (const locale of ['en', 'zh-CN'] as const) it(`keeps a native file request and session scope distinct from a narrow thread rule in ${locale}`, async () => {
  await setLocale(locale);
  const { approval } = draw({ request_kind: 'file_change', options: ALLOW_DENY, detail: { changes: [change('src/login.ts', '@@ -1 +1 @@\n-old\n+fixed\n')], reason: 'Keep the redirect within this site' } });
  expect(screen.getByText('Keep the redirect within this site')).toBeVisible();
  expect(screen.getAllByText('src/login.ts').length).toBeGreaterThan(0);
  expect(screen.getByRole('button', { name: `${i18n.t('approvals.allowSession')} · ${approval.title}` })).toBeVisible();
  expect(screen.queryByRole('button', { name: answerName('allowThread', approval.title) })).toBeNull();
  await userEvent.keyboard('{Escape}'); expect(wire.decide).not.toHaveBeenCalled();
});
