/** The screen of a thread has three parts and only the middle one scrolls: the head and the dock are on the screen at any scroll position. */
import { act, cleanup, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createPortal } from 'react-dom';
import { QueryClient } from '@tanstack/react-query';
import { viewedItemQuery } from '@/api/threadHistory';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { ThreadScreen } from '@/features/thread/Thread';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import type { Thread } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { LocalAddressRewrite } from '@/lib/markdown';
import { Markdown } from '@/ui/Markdown';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME, me, userRef } from '../doubles/fixtures';
import { agentMessage, threadInput, toolCall } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show } from './support';

vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: () => undefined, useOrgTreeFeed: () => undefined, useSocketStatus: () => ({ phase: 'ready' }) }));
vi.mock('@/shell/screen', async (load) => ({ ...await load<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
// The head draws the lines that tell of the thread into the place the screen gives it, as the real one does.
vi.mock('@/features/thread/Header', () => ({ ThreadHeader: ({ thread, notes, agent, onAgent }: { thread: Thread; notes?: HTMLElement | null; agent?: { name: string } | null; onAgent?: (step: string | null) => void }) => <>
  <header aria-label="Thread"><button onClick={() => onAgent?.(null)}>{thread.name}</button>{agent !== null && agent !== undefined && <h2>{agent.name}</h2>}</header>
  {notes !== null && notes !== undefined && createPortal(<p>Imported history</p>, notes)}
</> }));
vi.mock('@/features/composer/Composer', () => ({ Composer: () => <textarea aria-label="Thread draft" /> }));
vi.mock('@/features/composer/QueueTray', () => ({ QueueTray: ({ queued, sending }: { queued: readonly unknown[]; sending: readonly unknown[] }) => <p>{`The queue holds ${queued.length + sending.length}`}</p> }));
vi.mock('@/features/timeline/SubagentLane', () => ({ SubagentConversation: ({ itemId }: { itemId: string }) => <><p>{`The own conversation of ${itemId}`}</p><Markdown text="[Child app](http://localhost:43210)" /></> }));
vi.mock('@/features/schedules/Schedules', () => ({ PendingAgentSchedules: () => null }));
vi.mock('@/features/thread/ConnectionBanner', () => ({ ConnectionBanner: () => null }));
vi.mock('@/features/thread/BlockedBanner', () => ({ BlockedBanner: () => null }));
vi.mock('@/features/thread/LockBanner', () => ({ LockBanner: () => null }));
vi.mock('@/features/thread/ContinuationNotice', () => ({ ContinuationNotice: () => null }));
vi.mock('@/features/source/Preparation', () => ({ SourcePreparation: () => null }));
vi.mock('@/features/source/Review', () => ({ ReviewTray: () => null }));
vi.mock('@/features/gateways/ThreadGatewayStatus', () => ({ ThreadGatewayStatus: () => null }));
vi.mock('@/features/timeline/FindInConversation', () => ({ FindInConversation: () => null }));
// The line is told where it stands and what it is to carry; what it draws is the case of its own file.
vi.mock('@/features/timeline/StatusLine', () => ({ StatusLine: ({ withStop }: { withStop?: boolean }) => <p data-status-line data-with-stop={String(withStop ?? false)} /> }));
vi.mock('@/features/timeline/Timeline', () => ({ Timeline: () => <p>The conversation</p> }));
vi.mock('@/features/ports/LocalAddresses', () => ({ LocalAddresses: ({ environmentId, canView, children }: { environmentId: string | null; canView: boolean; children: ReactNode }) =>
  <LocalAddressRewrite.Provider value={environmentId !== null && canView ? () => `/resources/environments/${environmentId}?tab=ports&port=43210` : null}>{children}</LocalAddressRewrite.Provider> }));

let native: Thread;
beforeEach(() => {
  const world = new World(); const person = world.people.get('ada');
  const node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('A native thread is required');
  native = { ...world.threads.thread(person, node), state: 'idle' };
  threadWriter.reset(native.id, { thread: native, items: [{ ...agentMessage('answer', 'The redirect has been checked.'), thread_id: native.id, seq_created: 1, seq_updated: 1 }] }, false);
});
afterEach(async () => { cleanup(); threadWriter.evict(native.id); await setLocale('en'); });

function draw(width: number) {
  return show(<ViewCompactContext.Provider value={width === 400}><div style={{ width }}><ThreadScreen threadId={native.id} item={null} /></div></ViewCompactContext.Provider>);
}

it.each([400, 1440])('keeps the header and the composer outside the one area that scrolls, at %i px', (width) => {
  const view = draw(width);
  const frame = view.container.querySelector('[data-screen="thread"]');
  expect(frame).not.toBeNull();
  const areas = view.container.querySelectorAll('[data-scroll-area]');
  expect(areas).toHaveLength(1);
  const scroll = areas[0];
  const header = screen.getByRole('banner');
  const composer = screen.getByRole('textbox', { name: 'Thread draft' });
  const dock = view.container.querySelector('[data-thread-dock]');
  expect(scroll).toBeDefined();
  expect(scroll).not.toContainElement(header);
  expect(scroll).not.toContainElement(composer);
  expect(scroll).toContainElement(screen.getByText('The conversation'));
  // The queue stands in the dock with the composer, under the conversation, and is not carried away with it.
  expect(dock).toContainElement(composer);
  expect(dock).toContainElement(screen.getByText('The queue holds 0'));
  expect(scroll).not.toContainElement(dock as HTMLElement);
  // Three parts of the one screen, in this order: the head, the part that scrolls, the dock.
  const parts = [header, scroll!, dock!].map((part) => [...(frame?.querySelectorAll('*') ?? [])].indexOf(part));
  expect(parts).toEqual([...parts].sort((one, other) => one - other));
  expect(frame?.className).toContain('overflow-hidden');
});

it('draws the lines that tell of the thread at the top of the part that scrolls, not in the head', () => {
  const view = draw(1440);
  const line = screen.getByText('Imported history');
  const scroll = view.container.querySelector('[data-scroll-area]');
  expect(scroll).toContainElement(line);
  expect(screen.getByRole('banner')).not.toContainElement(line);
  expect(scroll?.firstElementChild).toContainElement(line);
});

const ADA = userRef(me(1, 'ada', 'Ada'));
const sentBlock = () => document.querySelector('[data-sent-message]');

it('names a directly linked older child after its parent step has been fetched into the shared query', () => {
  const step = { ...toolCall('old-review', { kind: 'subagent', child_stream: 'sub:older', name: 'Earlier reviewer', role: 'review', prompt: null, state: 'completed' }, { title: 'Review', status: 'completed' }), thread_id: native.id };
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(viewedItemQuery(native.id, step.id).queryKey, step);
  show(<ThreadScreen threadId={native.id} item={null} agent={step.id} />, cache);
  expect(screen.getByRole('heading', { name: 'Earlier reviewer', level: 2 })).toBeVisible();
  expect(document.querySelector('[data-agent-readonly]')).toHaveTextContent('Earlier reviewer');
});

it('draws a message sent to an idle thread as the person’s message in the conversation, dimmed, and not in the queue', () => {
  const view = draw(1440);
  expect(sentBlock()).toBeNull();
  act(() => threadWriter.apply(native.id, { inputs: [threadInput('first', 'Work in a new directory', { author: ADA, position: '1', clientId: 'sent-1' })] }));
  const block = sentBlock();
  expect(block).not.toBeNull();
  expect(view.container.querySelector('[data-scroll-area]')).toContainElement(block as HTMLElement);
  expect(block).toHaveTextContent('Work in a new directory');
  expect(block?.querySelector('.opacity-60')).not.toBeNull();
  expect(screen.getByText('The queue holds 0')).toBeVisible();
  // The agent takes it: the message is a row of the conversation, and the block that stood for it goes.
  act(() => threadWriter.apply(native.id, { inputs: [{ ...threadInput('first', 'Work in a new directory', { author: ADA, position: '1', clientId: 'sent-1' }), state: 'delivered', seq_updated: 1 }] }));
  expect(sentBlock()).toBeNull();
});

it('keeps a message that waits behind a turn that runs in the queue, and out of the conversation', () => {
  draw(1440);
  act(() => threadWriter.apply(native.id, { thread: { ...native, state: 'running', seq: native.seq + 1 }, inputs: [threadInput('behind', 'Then run the tests', { author: ADA, position: '1' })] }));
  expect(sentBlock()).toBeNull();
  expect(screen.getByText('The queue holds 1')).toBeVisible();
});

it('shows a sub-agent’s own conversation in place of the main one, read-only with a lock and no composer, and the title and Escape bring the main one back', async () => {
  const step = toolCall('review-call', { kind: 'subagent', child_stream: 'sub:reviewer', name: 'reviewer', role: 'review', prompt: null, state: 'running' }, { title: 'Review', status: 'in_progress' });
  act(() => threadWriter.apply(native.id, { items: [{ ...step, thread_id: native.id, seq_created: 5, seq_updated: 5 }] }));
  const leave = vi.fn();
  const view = show(<ThreadScreen threadId={native.id} item={null} agent="review-call" onAgent={leave} />);
  expect(screen.getByText('The own conversation of review-call')).toBeVisible();
  expect(screen.queryByText('The conversation')).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Thread draft' })).not.toBeInTheDocument();
  const lock = view.container.querySelector('[data-agent-readonly]');
  expect(lock).toHaveTextContent('Read-only: reviewer');
  expect(lock?.querySelector('svg')).not.toBeNull();
  expect(screen.getByRole('heading', { name: 'reviewer', level: 2 })).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Back to Main' }));
  expect(leave).toHaveBeenLastCalledWith(null);
  await userEvent.click(screen.getByRole('button', { name: native.name }));
  expect(leave).toHaveBeenCalledTimes(2);
  fireEvent.keyDown(document.body, { key: 'Escape' });
  expect(leave).toHaveBeenCalledTimes(3);
  // A question in a field keeps its own Escape.
  const field = document.createElement('input'); document.body.append(field); field.focus();
  fireEvent.keyDown(field, { key: 'Escape' });
  expect(leave).toHaveBeenCalledTimes(3);
  field.remove();
});

it('shows the main conversation, with its composer, where no sub-agent is named', () => {
  draw(1440);
  expect(screen.getByText('The conversation')).toBeVisible();
  expect(screen.getByRole('textbox', { name: 'Thread draft' })).toBeVisible();
  expect(document.querySelector('[data-agent-readonly]')).toBeNull();
});

it('gives the status line the Stop of a running turn only where the composer, which has it, is not drawn', () => {
  const main = draw(1440);
  expect(screen.getByRole('textbox', { name: 'Thread draft' })).toBeVisible();
  expect(document.querySelector('[data-status-line]')).toHaveAttribute('data-with-stop', 'false');
  main.unmount();
  const step = toolCall('review-call', { kind: 'subagent', child_stream: 'sub:reviewer', name: 'reviewer', role: 'review', prompt: null, state: 'running' }, { title: 'Review', status: 'in_progress' });
  act(() => threadWriter.apply(native.id, { items: [{ ...step, thread_id: native.id, seq_created: 5, seq_updated: 5 }] }));
  show(<ThreadScreen threadId={native.id} item={null} agent="review-call" onAgent={vi.fn()} />);
  expect(screen.queryByRole('textbox', { name: 'Thread draft' })).toBeNull();
  expect(document.querySelector('[data-status-line]')).toHaveAttribute('data-with-stop', 'true');
});

it.each([true, false])('keeps the environment’s authorized port links in a selected child conversation (Ports access: %s)', (canView) => {
  if (native.environment === null) throw new Error('The port-link case has its native environment');
  const environment = native.environment.id;
  native = { ...native, seq: native.seq + 1, environment_permissions: canView ? ['ports.view'] : [] };
  act(() => threadWriter.apply(native.id, { thread: native }));
  show(<ThreadScreen threadId={native.id} item={null} agent="review-call" onAgent={vi.fn()} />);
  expect(screen.getByRole('link', { name: 'Child app' })).toHaveAttribute('href', canView ? `/resources/environments/${environment}?tab=ports&port=43210` : 'http://localhost:43210');
});
