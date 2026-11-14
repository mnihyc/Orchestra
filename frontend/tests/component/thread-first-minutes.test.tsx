/** The first turn of a new thread: the beginning is one line, the harness's own state is drawn only under Diagnostics, and an answer said twice stands once. */
import { createRef, useImperativeHandle } from 'react';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Live from '@/realtime/live';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { Timeline, type TimelineHandle } from '@/features/timeline/Timeline';
import { setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import type { AgentRef, Item, MarkerPayload, ProviderRef } from '@/generated/api-types';
import { agentMessage, errorItem, marker, turn, userMessage } from '../doubles/thread-rows';
import { A, snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const wire = vi.hoisted((): { state: Record<string, unknown>; scrolled: string[] } => ({ state: {}, scrolled: [] }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve(wire.state), staleTime: Infinity },
  setMeState: () => Promise.resolve(),
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
// Geometry belongs to the browser controls; the list only has to draw every row.
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, scrollToKey: (key: string) => {
    const found = Array.from({ length: props.count }, (_, index) => props.keyOf(index)).includes(key);
    if (found) wire.scrolled.push(key);
    return found;
  }, isAtEnd: () => true, element: () => null }));
  return <ol aria-label={props.label}>{Array.from({ length: props.count }, (_, index) => <li key={props.keyOf(index)}>{props.render(index)}</li>)}</ol>;
} }));
vi.mock('@/features/timeline/StatusLine', () => ({ StatusLine: () => null }));

const session = client.session.getState();
const ids: string[] = [];
beforeEach(() => { wire.state = {}; wire.scrolled = []; });
afterEach(async () => {
  cleanup();
  for (const id of ids.splice(0)) threadWriter.evict(id);
  client.session.setState(session);
  await setLocale('en');
});

const AT = '2026-10-01T10:00:00.000Z';
const AGENT = { id: 'agent-1', name: 'Codex', address: 'acme.codex', disabled: false, harness: 'codex', harness_kind: 'codex', machine: null } as unknown as AgentRef;
const PROVIDER = { id: 'provider-1', name: 'OpenAI-API-R4', address: 'acme.openai', harness: null, harness_kind: null, kind: 'api', placement: 'managed', status: 'ready' } as unknown as ProviderRef;

function report(id: string, change: string, value: unknown, more: Record<string, unknown> = {}): Item {
  return marker(id, { marker: 'harness_changed', data: { program: 'Codex', change, subject: null, previous: { state: 'unsupported' }, value,
    expected: { state: 'unsupported' }, changed: false, diverged: false, next_turn: false, turn_id: null, reported_at: AT, ...more } } as unknown as MarkerPayload);
}

/** A new thread with one "hello", as the owner saw it, held by the store the way the timeline reads it. */
function held(said: Item[] = []): string {
  client.session.setState({ status: 'signed_in', userId: 'first-minutes-reader', readerVersion: 1 });
  const source = snapshot(), id = source.thread.id;
  ids.push(id);
  const done = turn('first-turn', { ordinal: 1, state: 'completed', inputId: 'hello-input' });
  const by = source.thread.owner;
  const items: Item[] = [
    marker('created', { marker: 'created', data: { by, agent: null, provider: null, environment: null, model: null, effort: null } }),
    marker('prepared', { marker: 'source_prepared', data: { by, directory: 'repo', branch: null, head: null, job_id: 'job-1' } }),
    marker('started', { marker: 'segment_started', data: { agent: AGENT, provider: PROVIDER, ordinal: 1, resumed: false, segment_id: 'segment-1' } } as unknown as MarkerPayload),
    report('mcp-starting', 'mcp_startup', { status: 'starting', message: null }, { subject: 'orch' }),
    report('mcp-ready', 'mcp_startup', { status: 'ready', message: null }, { subject: 'orch' }),
    report('mode', 'collaboration_mode', { mode: 'default', settings: { model: 'gpt-6-luna', reasoning_effort: 'medium' } }, { next_turn: true }),
    report('effort', 'effort', 'medium', { next_turn: true }),
    report('instructions', 'collaboration_instructions', null, { next_turn: true }),
    report('active', 'session_status', { type: 'active', active_flags: [] }),
    report('warning', 'unrecognized_event', { count: 1 }, { subject: 'warning' }),
    userMessage('hello', 'hello', { turn: done.id, author: by }),
    ...said,
    agentMessage('answer', 'the-answer', { turn: done.id }),
    report('read-state', 'unrecognized_event', { count: 1 }, { subject: 'thread/readState/changed' }),
    report('idle', 'session_status', { type: 'idle', active_flags: [] }, { changed: true }),
  ];
  threadWriter.reset(id, { ...source, turns: [done], items: items.map((item) => ({ ...item, thread_id: id, continuation_id: A })) }, false, captureThreadRead(id));
  return id;
}

const drawn = (view: { container: HTMLElement }): string => view.container.textContent;
const MACHINERY = /MCP server status|session status|collaboration mode|collaboration instructions|Unrecognized|Applies to future turns|its effort/;

it('draws the beginning as one line, then his message and the answer, in Full', async () => {
  const id = held();
  const view = show(<Timeline threadId={id} item={null} empty={null} />);
  await waitFor(() => expect(screen.getByText('hello')).toBeVisible());
  expect(screen.getByText('Started with Codex on OpenAI-API-R4')).toBeVisible();
  expect(screen.getByText('the-answer')).toBeVisible();
  expect(drawn(view)).not.toMatch(MACHINERY);
  expect(drawn(view)).not.toMatch(/created this thread|prepared the source/);
});

it('opens the start line into the lines it stands for, with its creator and time under the pointer', async () => {
  const id = held();
  show(<Timeline threadId={id} item={null} empty={null} />);
  const line = await screen.findByRole('button', { name: /Started with Codex on OpenAI-API-R4/ });
  expect(line).toHaveAttribute('title', expect.stringMatching(/^Created by .+ · /));
  expect(line).toHaveAttribute('aria-expanded', 'false');
  await userEvent.click(line);
  expect(line).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByText(/created this thread/)).toBeVisible();
  expect(screen.getByText(/prepared the source in repo/)).toBeVisible();
  expect(screen.getByText(/Started Codex with OpenAI-API-R4/)).toBeVisible();
});

it('draws the same turn as a message and an answer in Focused and Live', async () => {
  for (const timeline of [{ work_detail: 'summary' }, { work_detail: 'steps', live_tools: true }]) {
    wire.state = { timeline };
    const id = held();
    const view = show(<Timeline threadId={id} item={null} empty={null} />);
    await waitFor(() => expect(screen.getByText('the-answer')).toBeVisible());
    expect(screen.getByText('Started with Codex on OpenAI-API-R4')).toBeVisible();
    expect(drawn(view)).not.toMatch(MACHINERY);
    cleanup(); threadWriter.evict(id); ids.pop();
  }
});

it('draws every line of the turn under Diagnostics, each with its time under the pointer', async () => {
  const id = held();
  wire.state = { timeline: { threads: { [id]: { work_detail: 'steps', live_tools: false, diagnostics: true } } } };
  const view = show(<Timeline threadId={id} item={null} empty={null} />);
  await waitFor(() => expect(screen.getAllByText(/Unrecognized Codex event/).length).toBe(2));
  expect(screen.getByText(/Unrecognized Codex event warning/)).toBeVisible();
  expect(screen.getByText(/Unrecognized Codex event thread\/readState\/changed/)).toBeVisible();
  expect(screen.getAllByText(/MCP server status/).length).toBeGreaterThanOrEqual(2);
  expect(screen.getByText(/reported its collaboration mode as/)).toBeVisible();
  expect(screen.getByText(/reported its session status as Active/)).toBeVisible();
  expect(screen.getByText(/changed its session status to Idle/)).toBeVisible();
  expect(screen.getByText(/created this thread/)).toBeVisible();
  expect(screen.getByText(/prepared the source in repo/)).toBeVisible();
  expect(screen.getByText('hello')).toBeVisible();
  expect(screen.getByText('the-answer')).toBeVisible();
  expect(screen.queryByText('Started with Codex on OpenAI-API-R4')).toBeNull();
  const line = screen.getByText(/reported its session status as Active/).closest('[title]');
  expect(line).not.toBeNull();
  expect(line?.querySelector('time')).toBeNull();
  expect(drawn(view)).not.toMatch(/Started with Codex/);
});

it('draws an answer that was said twice once', async () => {
  const done = 'first-turn';
  const id = held([agentMessage('said-first', 'the-answer', { turn: done, phase: 'commentary' })]);
  show(<Timeline threadId={id} item={null} empty={null} />);
  await waitFor(() => expect(screen.getByText('the-answer')).toBeVisible());
  expect(screen.getAllByText('the-answer')).toHaveLength(1);
});

function failedStartup(): Item[] {
  const notice = errorItem('paired-notice', 'The server failed', { code: 'mcp_server_failed', turn: 'first-turn' });
  if (notice.kind !== 'error') throw new Error('The fixture is an error');
  notice.payload.details = { turn_id: 'first-turn', change: 'mcp_startup', server: 'Source catalog', message: 'Credentials expired' };
  return [report('failed-report', 'mcp_startup', { status: 'failed', message: 'Credentials expired' }, { turn_id: 'first-turn', subject: 'Source catalog' }), notice];
}

it('reveals the retained notice selected by Find without enabling unrelated diagnostics', async () => {
  const id = held(failedStartup());
  const ref = createRef<TimelineHandle>();
  const view = show(<Timeline ref={ref} threadId={id} item={null} empty={null} />);
  await waitFor(() => expect(screen.getByText('the-answer')).toBeVisible());
  expect(view.container.querySelector('[data-item="paired-notice"]')).toBeNull();
  act(() => ref.current?.scrollToItem('paired-notice'));
  await waitFor(() => expect(view.container.querySelector('[data-item="paired-notice"]')).toBeVisible());
  await waitFor(() => expect(wire.scrolled).toContain('paired-notice'));
  expect(screen.queryByText(/reported its session status as Active/)).toBeNull();
});

it('opens a copied link to a suppressed notice at that notice', async () => {
  const id = held(failedStartup());
  const view = show(<Timeline threadId={id} item="paired-notice" empty={null} />);
  await waitFor(() => expect(view.container.querySelector('[data-item="paired-notice"]')).toBeVisible());
  await waitFor(() => expect(wire.scrolled).toContain('paired-notice'));
});

it('opens a copied start-marker link at its own visible row', async () => {
  const id = held();
  show(<Timeline threadId={id} item="prepared" empty={null} />);
  await waitFor(() => expect(screen.getByText(/prepared the source in repo/)).toBeVisible());
  await waitFor(() => expect(wire.scrolled).toContain('prepared'));
});

it('keeps previous Find targets visible while lighting only the latest selection', async () => {
  const id = held(failedStartup());
  const ref = createRef<TimelineHandle>();
  const view = show(<Timeline ref={ref} threadId={id} item={null} empty={null} />);
  await waitFor(() => expect(screen.getByText('the-answer')).toBeVisible());
  act(() => ref.current?.scrollToItem('answer'));
  await waitFor(() => expect(view.container.querySelector('[data-item="answer"]')?.className).toMatch(/ring-accent/));
  act(() => ref.current?.scrollToItem('paired-notice'));
  await waitFor(() => expect(view.container.querySelector('[data-item="paired-notice"]')?.className).toMatch(/ring-accent/));
  expect(view.container.querySelector('[data-item="answer"]')?.className).not.toMatch(/ring-accent/);
});
