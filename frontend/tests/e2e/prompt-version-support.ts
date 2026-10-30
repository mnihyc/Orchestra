/** Native-shaped continuation responses for production-browser reader tests; no model calls. */
import type { Page } from '@playwright/test';
import type { ContentPart, Item, SearchHit, ThreadContinuationProjection, ThreadOpening, ThreadPromptSummary, ThreadPromptVersion, ThreadInput } from '../../src/generated/api-types';
import { agentMessage, command, threadInput, userMessage } from '../doubles/thread-rows';
import { ACME } from './support';
import { MAIN } from './inspector-support';

export const A = '01950000-0000-7000-8000-000000000001', B = '01950000-0000-7000-8000-000000000002';
export const INPUT_A = '01950000-0000-7000-8001-000000000001', INPUT_B = '01950000-0000-7000-8001-000000000002';
export const ITEM_A = '01950000-0000-7000-8002-000000000001', ITEM_B = '01950000-0000-7000-8002-000000000002';
export const SHARED_ITEM = '01950000-0000-7000-8002-000000000003', COMMAND_ITEM = '01950000-0000-7000-8002-000000000004';
export const ROOT_ITEM = '01950000-0000-7000-8002-000000000005';

export async function versions(page: Page, existing = true, options: { windowed?: boolean; searchView?: string; shared?: boolean; failed?: boolean } = {}) {
  let active = existing && options.failed !== true ? B : A, total = existing ? 2 : 1, execution = total, sequence = 1000;
  let status: ThreadPromptVersion['status'] = options.failed === true ? 'failed' : 'ready';
  let pendingState: ThreadInput['state'] = 'pending', queueRevision = 8;
  let latestThread: ThreadOpening['sync']['thread'] | null = null;
  let connections = 0;
  const subscriptionsSeen: { id: number; feed: string; continuation: string | null }[] = [];
  const publishers = new Set<() => void>();
  let revised: ContentPart[] = [{ type: 'text', text: 'Revised release plan' }];
  let author: ThreadPromptSummary['author'] = null;
  const calls: { method: string; path: string; query: string; body: unknown }[] = [];
  const projection = (id: string | null): ThreadContinuationProjection => ({ viewed_continuation_id: id ?? active,
    active_continuation_id: active, execution_revision: execution, projection_revision: id === null ? execution : 1, follows_active: id === null });
  const summary = (id: string): ThreadPromptSummary => ({ input_id: id === A ? INPUT_A : INPUT_B, group_id: INPUT_A,
    continuation_id: id, ordinal: id === A ? 1 : 2, total, author, edit: { available: true, reason: null, detail: null } });
  const version = (id: string): ThreadPromptVersion => ({ ...summary(id), id: id === A ? INPUT_A : INPUT_B,
    created_at: '2026-10-17T00:00:00Z', status: id === A ? 'ready' : status,
    job: id === B && status === 'failed' ? { id: 'failed-preparation', kind: 'thread_continuation', phase: 'failed', percent: 100, state: 'failed' } : null,
    item_id: id === A ? ITEM_A : status === 'ready' ? ITEM_B : null });
  const pending = (): ThreadInput => ({ ...threadInput(INPUT_B, '', { author, position: '1', state: pendingState }),
    thread_id: MAIN, continuation_id: B, content: revised, queue_held: true, seq_updated: sequence });
  const queue = (id: string) => options.failed === true && id === B && pendingState === 'pending' ? [pending()] : [];
  const contents = (id: string): Item[] => {
    if (id === B && status !== 'ready') return [];
    if (author === null) throw new Error('The native opening supplies the actual author first');
    const prompt = userMessage(id === A ? ITEM_A : ITEM_B, '', { author, inputId: summary(id).input_id,
      parts: id === A ? [{ type: 'text', text: 'Original release plan' }] : revised });
    const base: Item[] = [prompt];
    if (options.shared === true || options.windowed === true) {
      base.push(agentMessage(SHARED_ITEM, id === A ? 'Original source keeps its own text' : 'Frozen source keeps its own text'));
      const tool = command(COMMAND_ITEM, { command: 'cat report.txt', output: id === A ? 'Original output preview' : 'Frozen output preview', exit: 0 });
      if (tool.kind !== 'tool_call' || tool.payload.tool.kind !== 'command') throw new Error('Expected command fixture');
      tool.payload.tool.output = { ...tool.payload.tool.output, total_len: 1000, truncated: true };
      base.push(tool, agentMessage(ROOT_ITEM, 'The main view stays at this anchor'));
    }
    if (options.windowed === true) base.push(...Array.from({ length: 220 }, (_, n) => agentMessage(`later-${id}-${n}`, `Later history item ${n}`)));
    return base.map((item, n) => ({ ...item, thread_id: MAIN, continuation_id: item.id === ITEM_A || item.id === ITEM_B ? id : A,
      seq_created: n + 1, seq_updated: n + 1 }));
  };
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (ws) => {
    connections += 1;
    const server = ws.connectToServer(), subscriptions = new Map<number, { selection: string | null; seq: number }>();
    const publish = () => {
      for (const [id, held] of subscriptions) {
        ws.send(JSON.stringify({ op: 'sync', id, feed: `thread:${MAIN}`, prev: held.seq, seq: sequence,
          data: { projection: projection(held.selection), ...(latestThread === null ? {} : {
            thread: { ...latestThread, seq: sequence, active_continuation_id: active, execution_revision: execution },
          }) } }));
        held.seq = sequence;
      }
    };
    publishers.add(publish);
    ws.onMessage((raw) => {
      const frame = JSON.parse(String(raw)) as { op: string; id?: number; feed?: string; continuation_id?: string; since?: number };
      if (frame.op === 'sub' && frame.feed === `thread:${MAIN}` && frame.id !== undefined) {
        subscriptionsSeen.push({ id: frame.id, feed: frame.feed, continuation: frame.continuation_id ?? null });
        subscriptions.set(frame.id, { selection: frame.continuation_id ?? null, seq: frame.since ?? sequence });
        ws.send(JSON.stringify({ op: 'subscribed', id: frame.id, feed: frame.feed, mode: 'delta', seq: frame.since ?? sequence,
          projection: projection(frame.continuation_id ?? null) }));
      } else if (frame.op === 'unsub' && frame.id !== undefined && subscriptions.delete(frame.id)) {
        ws.send(JSON.stringify({ op: 'closed', id: frame.id, feed: `thread:${MAIN}`, reason: 'unsubscribed' }));
      } else server.send(raw);
    });
    ws.onClose((code, reason) => { publishers.delete(publish); void server.close({ ...(code === undefined ? {} : { code }), ...(reason === undefined ? {} : { reason }) }); });
  });
  await page.route(new RegExp(`/api/v1/threads/${MAIN}/(?:opening|read|items(?:/[^/?]+(?:/output)?)?|inputs(?:/[^/]+(?:/versions)?)?|continuations/[^/]+/activate)(?:\\?|$)`), async (route) => {
    const request = route.request(), url = new URL(request.url());
    const body: unknown = request.method() === 'POST' || request.method() === 'PATCH' ? request.postDataJSON() : null;
    calls.push({ method: request.method(), path: url.pathname, query: url.search, body });
    const scoped = projection(url.searchParams.get('continuation_id'));
    if (url.pathname.endsWith('/opening')) {
      const response = await route.fetch();
      const data = await response.json() as ThreadOpening;
      author = data.sync.thread.owner;
      const id = scoped.viewed_continuation_id;
      const prompt = summary(id);
      const items = contents(id);
      data.sync = { ...data.sync, seq: sequence, projection: scoped, viewed_prompt: id === A ? null : version(id),
        thread: { ...data.sync.thread, seq: sequence, state: 'idle', current_turn_id: null, queue: { ...data.sync.thread.queue, revision: 8 },
          active_continuation_id: active, execution_revision: execution },
        items: items.slice(-200), turns: [], segments: [], inputs: queue(id),
        prompt_summaries: [prompt], inputs_next_after: null, inputs_queue_revision: queueRevision, complete: true, has_older: items.length > 200 };
      data.outline = { entries: [], next_before: null, seq: sequence, projection: scoped };
      latestThread = data.sync.thread;
      await route.fulfill({ json: data }); return;
    }
    if (url.pathname.endsWith(`/inputs/${INPUT_B}`)) {
      if (request.method() === 'PATCH') revised = (body as { content: ContentPart[] }).content;
      else if (request.method() === 'DELETE') pendingState = 'cancelled';
      else throw new Error('Unexpected pending-input fixture operation');
      sequence += 1; queueRevision += 1;
      await route.fulfill({ json: request.method() === 'PATCH' ? { input: pending(), inputs: [] } : pending() }); return;
    }
    if (url.pathname.endsWith('/inputs')) {
      await route.fulfill({ json: { inputs: queue(scoped.viewed_continuation_id), projection: scoped,
        queue_revision: queueRevision, seq: sequence, next_after: null } }); return;
    }
    if (url.pathname.endsWith('/read')) {
      const ack = body as { seq: number; continuation_id?: string };
      await route.fulfill({ json: { last_read_seq: ack.seq, projection: projection(ack.continuation_id ?? null) } }); return;
    }
    if (url.pathname.endsWith('/output')) {
      await route.fulfill({ contentType: 'text/plain', body: scoped.viewed_continuation_id === A ? 'Complete original output' : 'Complete frozen output' }); return;
    }
    if (url.pathname.endsWith('/items')) {
      const all = contents(scoped.viewed_continuation_id);
      const around = url.searchParams.get('around');
      const at = around === null ? -1 : all.findIndex((item) => item.id === around);
      const items = at < 0 ? [] : all.slice(Math.max(0, at - 1), at + 2);
      await route.fulfill({ json: { projection: scoped, items, turns: [], segments: [],
        prompt_summaries: [summary(scoped.viewed_continuation_id)], has_older: at > 1, has_newer: at >= 0 && at + 2 < all.length } }); return;
    }
    if (url.pathname.includes('/items/')) {
      const item = contents(scoped.viewed_continuation_id).find((one) => one.id === url.pathname.split('/').at(-1));
      await route.fulfill(item === undefined ? { status: 404, json: { code: 'not_found', status: 404 } } : { json: item }); return;
    }
    if (request.method() === 'GET') {
      const next = url.searchParams.has('before') ? A : B;
      await route.fulfill({ json: { projection: scoped, active_continuation_id: active, group_id: INPUT_A,
        next_after: null, next_before: null, rows: [version(next)], selected_version: summary(scoped.viewed_continuation_id).ordinal, total, seq: sequence } }); return;
    }
    if (url.pathname.endsWith('/versions')) {
      revised = (body as { content: ContentPart[] }).content; total = 2; execution += 1; active = B; sequence += 1;
    } else {
      active = url.pathname.split('/').at(-2) ?? active; execution += 1; sequence += 1;
      if (options.failed === true && active === B) { status = 'ready'; pendingState = 'delivered'; }
      for (const publish of publishers) publish();
    }
    await route.fulfill({ status: 202, json: { continuation_id: active, input_id: INPUT_B, operation_id: 'synthetic-edit', seq: sequence,
      job: { id: 'synthetic-edit', kind: 'thread_continuation', phase: 'preparing', percent: 0, state: 'running' } } });
  });
  if (options.searchView !== undefined) await page.route(/\/api\/v1\/search(?:\?|$)/, async (route) => {
    const url = new URL(route.request().url());
    calls.push({ method: 'GET', path: url.pathname, query: url.search, body: null });
    const id = url.searchParams.get('scope') === MAIN ? url.searchParams.get('continuation_id') ?? active : options.searchView ?? active;
    const item = contents(id).find((row) => row.id === SHARED_ITEM);
    if (item === undefined) throw new Error('Search fixture needs the retained shared item');
    const hit: Extract<SearchHit, { kind: 'item' }> = { kind: 'item', item, projection: projection(id), snippet: 'unique retained source', matches: [],
      thread: { id: MAIN, kind: 'thread', name: 'Main thread', redacted: false },
      topic: { id: ACME.fixLogin, kind: 'topic', name: 'Fix login', redacted: false },
      project: { id: ACME.web, kind: 'project', name: 'Web', redacted: false } };
    await route.fulfill({ json: { items: [hit], next_cursor: null, partial: false } });
  });
  return { calls, subscriptions: subscriptionsSeen, connections: () => connections };
}
