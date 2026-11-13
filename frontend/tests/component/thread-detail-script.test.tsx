/** Real preference controls drive the real row planner and renderers through a scripted native turn. */
import { useImperativeHandle } from 'react';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Live from '@/realtime/live';
import { DetailControl } from '@/features/thread/DetailControl';
import { PinnedApprovals } from '@/features/thread/PinnedApprovals';
import { Timeline } from '@/features/timeline/Timeline';
import { useTimelinePreferences } from '@/features/timeline/preferences';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { ALLOW_DENY, agentMessage, approvalItem, change, command, fileChange, harnessApproval, reasoning, turn, userMessage } from '../doubles/thread-rows';
import { A, snapshot } from '../unit/continuation-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const wire = vi.hoisted(() => { const state: Record<string, unknown> = {}; return { state }; });
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve(wire.state), staleTime: Infinity },
  setMeState: (key: string, value: unknown) => { wire.state = { ...wire.state, [key]: value }; return Promise.resolve(); },
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
// Geometry belongs to the browser controls; render every planned row to inspect its actual content here.
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, scrollToKey: () => true, isAtEnd: () => true, element: () => null }));
  return <ol aria-label={props.label}>{Array.from({ length: props.count }, (_, index) => <li key={props.keyOf(index)}>{props.render(index)}</li>)}</ol>;
} }));
vi.mock('@/features/timeline/StatusLine', () => ({ StatusLine: () => null }));
const appearance = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
const session = client.session.getState();
const ids: string[] = [];
afterEach(async () => {
  cleanup(); for (const id of ids.splice(0)) threadWriter.evict(id); client.session.setState(session); useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});
function Detail({ id }: { id: string }) { return <DetailControl preferences={useTimelinePreferences(id)} />; }

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`keeps input, answer and one pending card through Full, Focused and Live at ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    wire.state = {}; client.session.setState({ status: 'signed_in', userId: 'script-reader', readerVersion: 1 });
    const source = snapshot(); const id = source.thread.id; ids.push(id);
    const tag = `${locale}-${theme}-${width}`; const current = turn(`script-${tag}`, { ordinal: 1, state: 'running', inputId: 'input' });
    const cmd = command(`command-${tag}`, { turn: current.id, command: 'check files', output: 'Current streamed output', exit: null });
    const approval = harnessApproval(`request-${tag}`, { threadId: id, itemId: `ask-${tag}`, requester: source.thread.owner, title: 'Inspect changes',
      request: { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'inspect files', cwd: '/work', reason: 'Check the result before continuing' } } });
    const ask = approvalItem(`ask-${tag}`, approval.id, { turn: current.id });
    if (ask.kind !== 'approval') throw new Error('The script needs its native approval');
    threadWriter.reset(id, { ...source, thread: { ...source.thread, state: 'running', current_turn_id: current.id }, turns: [current], items: [
      userMessage(`input-${tag}`, 'Repair the redirect', { turn: current.id, author: source.thread.owner }),
      reasoning(`thought-${tag}`, 'Inspect the incoming request', { turn: current.id }), cmd,
      fileChange(`edit-${tag}`, { turn: current.id, changes: [change('src/login.ts', '@@ -1 +1 @@\n-old\n+fixed')] }),
      { ...ask, payload: { approval_id: approval.id, approval } }, agentMessage(`answer-${tag}`, 'The redirect is repaired.', { turn: current.id }),
    ].map((item) => ({ ...item, thread_id: id, continuation_id: A })) }, false, captureThreadRead(id));
    const view = show(<ViewCompactContext.Provider value={width === 400}><div data-screen="thread" style={{ width }}><Detail id={id} /><Timeline threadId={id} item={null} empty={null} pinApprovals /><PinnedApprovals threadId={id} /></div></ViewCompactContext.Provider>);
    await waitFor(() => expect(screen.getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.full')));
    expect(view.container.querySelector(`[data-item="${cmd.id}"]`)).not.toBeNull();
    expect(screen.queryByText('Current streamed output')).not.toBeInTheDocument();
    const choose = async (mode: 'focused' | 'live') => { await userEvent.click(screen.getByRole('combobox')); await userEvent.click(screen.getByRole('option', { name: i18n.t(`thread.viewModes.${mode}`) })); };
    await choose('focused');
    await waitFor(() => expect(view.container.querySelector(`[data-fold="${current.id}"]`)).toHaveTextContent(i18n.t('thread.viewModes.toolCalls', { count: 2 })));
    expect(view.container.querySelector(`[data-item="${cmd.id}"]`)).toBeNull();
    expect(screen.getByText('Repair the redirect')).toBeVisible(); expect(screen.getByText('The redirect is repaired.')).toBeVisible();
    expect(screen.getAllByRole('button', { name: `${i18n.t('approvals.allowOnce')} · inspect files` })).toHaveLength(1);
    expect(view.container.querySelector(`[data-fold="${current.id}"]`)).toHaveTextContent(i18n.t('thread.viewModes.files', { count: 1 }));
    await choose('live');
    expect(await screen.findByText('Current streamed output')).toBeVisible();
    const next = command(cmd.id, { turn: current.id, command: 'check files', output: 'Current streamed output\nNext streamed line', exit: null });
    act(() => threadWriter.apply(id, { items: [{ ...next, thread_id: id, continuation_id: A, seq_updated: source.seq + 1 }] }, captureThreadRead(id)));
    expect(await screen.findByText(/Next streamed line/)).toBeVisible();
    expect(screen.getAllByRole('button', { name: `${i18n.t('approvals.allowOnce')} · inspect files` })).toHaveLength(1);
  });
}
