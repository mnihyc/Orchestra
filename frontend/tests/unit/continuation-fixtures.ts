/** Synthetic complete native rows plus explicit transport barriers; no real account/provider data. */
import type { Item, ThreadContinuationProjection, ThreadOpening, ThreadPromptSummary, ThreadPromptVersion, ThreadSync } from '@/generated/api-types';
import type { ClientFrame, SocketLike } from '@/realtime/socket';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { agentMessage, AT } from '../doubles/thread-rows';

export const A = '01950000-0000-7000-8000-000000000001';
export const B = '01950000-0000-7000-8000-000000000002';
export const C = '01950000-0000-7000-8000-000000000003';

export const projection = (viewed = A, active = viewed, revision = 1, follows = true): ThreadContinuationProjection => ({
  viewed_continuation_id: viewed, active_continuation_id: active, projection_revision: revision,
  execution_revision: revision, follows_active: follows,
});

export function snapshot(scope: ThreadContinuationProjection | null = projection(), seq = 10): ThreadSync {
  const world = new World();
  const person = world.people.get('ada');
  const row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The fixture needs its complete native thread');
  return { thread: { ...world.threads.thread(person, row), seq, my_last_read_seq: 0 }, seq, since: null,
    complete: true, items: [], turns: [], inputs: [], segments: [], has_older: false,
    ...(scope === null ? {} : { projection: scope, inputs_queue_revision: 1 }),
  };
}

export function opening(sync: ThreadSync): ThreadOpening {
  return { sync, draft: null, parameters: null, limits: null, settings: [],
    outline: { entries: [], next_before: null, seq: sync.seq,
      ...(sync.projection === undefined ? {} : { projection: sync.projection }) },
  };
}

export function answer(id: string, text: string, owner = A, seq = 10): Item {
  return { ...agentMessage(id, text, { status: 'in_progress' }), continuation_id: owner, seq_created: seq, seq_updated: seq };
}

export function summary(input: string, owner = A): ThreadPromptSummary {
  return { input_id: input, group_id: 'synthetic-group', continuation_id: owner, ordinal: 1, total: 2,
    author: null, edit: { available: false, reason: 'not_author', detail: null } };
}

export function viewedPrompt(status: ThreadPromptVersion['status'], owner = B): ThreadPromptVersion {
  return { id: 'edited-input', input_id: 'edited-input', item_id: null, continuation_id: owner, ordinal: 2, author: null,
    created_at: AT, edit: { available: false, reason: 'transition_in_progress', detail: null }, status,
    job: { id: 'synthetic-operation', kind: 'thread_continuation', phase: status, percent: status === 'preparing' ? 20 : 100,
      state: status === 'failed' ? 'failed' : status === 'ready' ? 'succeeded' : 'running' },
    // The current native source adds these fields to the already exported version type.
    ...{ group_id: 'synthetic-group', total: 2 },
  };
}

export function deferred<Value>() {
  let resolve: (value: Value) => void = () => { throw new Error('Deferred initialization missing'); };
  let reject: (error: Error) => void = () => { throw new Error('Deferred initialization missing'); };
  const promise = new Promise<Value>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

export class Signals<Value> {
  private readonly values: Value[] = [];
  private readonly waits: ((value: Value) => void)[] = [];
  emit(value: Value): void {
    const next = this.waits.shift();
    if (next === undefined) this.values.push(value); else next(value);
  }
  next(): Promise<Value> {
    const value = this.values.shift();
    return value === undefined ? new Promise((resolve) => this.waits.push(resolve)) : Promise.resolve(value);
  }
}

export class Wire implements SocketLike {
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  readonly sent: ClientFrame[] = [];
  readonly subscriptions = new Signals<Extract<ClientFrame, { op: 'sub' }>>();
  send(data: string): void {
    const frame = JSON.parse(data) as ClientFrame;
    this.sent.push(frame);
    if (frame.op === 'sub') this.subscriptions.emit(frame);
  }
  close(): void { this.readyState = 3; }
  ready(): void {
    this.readyState = 1; this.onopen?.();
    this.say({ op: 'ready', connection_id: 'synthetic', user_id: 'synthetic-reader', server_time: '',
      access_expires_at: '', heartbeat_secs: 25, build: 'synthetic' });
  }
  say(frame: Record<string, unknown>): void { this.onmessage?.({ data: JSON.stringify(frame) }); }
}
