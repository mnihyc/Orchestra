import { executionDisplay, display } from './thread-display.ts';
/** What the fake API says of threads: their rows and the requests that read and change them, the approvals in them, the thread that the dialog makes, and the feed of a thread.
 *
 * What a thread runs on (its provider, its agent profile, its environment, its model) is not kept here:
 * the resources of the world say it (`Resources.bindingOf`), so that the thread, the page of its
 * environment and the page of its provider tell one story, and the dialog that starts a thread chooses
 * among the lists the resources answer.
 *
 * A thread keeps its rows (items, turns, inputs, segments) with the sequence at which each last
 * changed, as the real system does. A test makes things happen in a thread by emitting rows through
 * `/__double/thread/emit`: they are kept, and sent as one `sync` frame to whoever follows the thread
 * (or only kept, to make the frame "get lost"). What the fake does itself is what a person's own
 * requests do: queue an input, take it back, stop a turn, decide an approval, rename, bell, draft.
 */
import { randomUUID } from 'node:crypto';

import type {
  AgentRef,
  Approval,
  ArrangementType,
  Draft,
  EnvironmentRef,
  Item,
  Node,
  ParameterView,
  PresenceUser,
  ProviderRef,
  ProviderModel,
  ResolvedSetting,
  Segment,
  Thread,
  ThreadData,
  ThreadInput,
  ThreadInputUpdateReply,
  ThreadLimits,
  ThreadSync,
  TreeNode,
  Turn,
} from '../../src/generated/api-types.ts';
import { contextOf, turnCostOf, turnUsagePage } from './context.ts';
import { ACME, MADE_AT, MODEL_ESTIMATE, userRef } from './fixtures.ts';
import { ACCESS_FEATURE, parameterViews, type ThreadChoices } from './model.ts';
import type { Places } from './places.ts';
import { type Answer, type Part, type Person, Refusal, type Subscriber, type World } from './world.ts';

/** The rows a test or a request puts into a thread; the fake fills in the rest. */
export interface Rows {
  thread?: Partial<Thread>;
  items?: Item[];
  turns?: Turn[];
  inputs?: ThreadInput[];
  segments?: Segment[];
}

interface Follower {
  id: number;
  /** The `seq` of the frame sent last to this subscription. */
  last: number;
  user: string;
  visible: boolean;
}

interface Held {
  seq: number;
  items: Map<string, Item>;
  turns: Map<string, Turn>;
  inputs: Map<string, ThreadInput>;
  segments: Map<string, Segment>;
  /** Fields of the thread row that the fake keeps besides what the tree and the places say. */
  fields: Partial<Thread>;
  clientIds: Map<string, string>;
  drafts: Map<string, Draft>;
  read: Map<string, number>;
  followers: Map<Subscriber, Map<number, Follower>>;
  presence: PresenceUser[];
  cannotSteer: boolean;
  limits: ThreadLimits;
  position: number;
  defaultDelivery: 'queue' | 'steer' | 'interrupt';
  /** The thread's own choices among the parameters of its agent (`agent.params`), and how many times they were written. */
  params: Record<string, unknown>;
  paramsRevision: number;
  /** A lock above the thread on its choices, for a test of a thread that cannot change them. */
  lockedBy: ThreadChoices['lockedBy'];
}

/** One connection may independently subscribe to the same native thread more than once. */
function* followers(held: Held): Generator<readonly [Subscriber, Follower]> {
  for (const [subscriber, subscriptions] of held.followers) for (const follower of subscriptions.values()) yield [subscriber, follower] as const;
}

/** What a thread says of why it is blocked, from what the tree says. */
const blockedFor = (reason: NonNullable<TreeNode['thread']>['blocked_reason'] | undefined): Thread['blocked'] => (reason === undefined || reason === null ? null : { detail: null, reason });

/** The name a model is shown under: the one it was given, or the name it is sent under when it was given none. */
const displayOf = (given: string | undefined, name: string): string => (given === undefined || given === '' ? name : given);
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** A row that has to be there: the request that asked for it has been shown to be about something that is. */
function must<T>(value: T | undefined, what: string): T {
  if (value === undefined) throw new Refusal(404, 'not_found', `there is no such ${what}`);
  return value;
}

/** The seeded thread whose actual binding supplies the model-control provider. */
const THREAD_PROVIDER = ACME.mainThread;

const SEGMENT = (threadId: string) => `00000000-0000-4000-8${(0x40).toString(16).padStart(3, '0')}-${threadId.slice(-12)}`;

export class ThreadWorld implements Part {
  readonly world: World;
  private threads = new Map<string, Held>();
  private settingRows = new Map<string, Map<string, { value: unknown; revision: number; by: Person['me'] }>>();
  approvals = new Map<string, Approval>();
  /** Features enabled by the place for model-derived parameters. */
  features: Record<string, boolean> = { [ACCESS_FEATURE]: true };
  created = 0;

  constructor(world: World) {
    this.world = world;
  }

  private get places(): Places {
    return this.world.places;
  }

  // ---------------------------------------------------------------- what a thread keeps

  private held(threadId: string): Held {
    let held = this.threads.get(threadId);
    if (held === undefined) {
      held = {
        seq: 100,
        items: new Map(),
        turns: new Map(),
        inputs: new Map(),
        segments: new Map(),
        fields: {},
        clientIds: new Map(),
        drafts: new Map(),
        read: new Map(),
        followers: new Map(),
        presence: [],
        cannotSteer: false,
        limits: { bypass_all: false, reached: [] },
        position: 0,
        defaultDelivery: 'queue',
        params: {},
        paramsRevision: 0,
        lockedBy: null,
      };
      this.threads.set(threadId, held);
    }
    return held;
  }

  private rowOf(threadId: string): TreeNode {
    const row = this.places.rowOf(threadId);
    if (row?.kind !== 'thread') throw new Refusal(404, 'not_found', 'there is no such thread, or it was deleted');
    return row;
  }

  /** The thread as the API gives it to a person. */
  thread(person: Person, row: TreeNode): Thread {
    const held = this.held(row.id);
    const base = this.places.base(person, row);
    const meta = this.places.metaOf(row.id);
    const resources = this.world.resources;
    const bound = resources.bindingOf(row.id);
    const refs = resources.refsOf(bound);
    const treeThread = row.thread;
    const turns = [...held.turns.values()];
    const running = turns.find((entry) => entry.state === 'running' || entry.state === 'starting');
    const pendingInputs = [...held.inputs.values()].filter((input) => input.state === 'pending' || input.state === 'delivering').length;
    const mayInteract = base.access.permissions.includes('thread.interact');
    const owner = this.places.userOfName(meta.owner);
    const made: Thread = {
      ...base,
      kind: 'thread',
      activity_seq: treeThread?.activity_seq ?? 1,
      binding:
        refs.provider === null || refs.agent === null
          ? null
          : {
              agent: refs.agent,
              arrangement: bound.arrangement ?? 'separated',
              // A thread made with an arrangement of its own says what that comes to; the threads the world began with say the same as ever.
              isolation: resources.isolationOf(bound) ?? {
                credential_path: 'native',
                executor: null,
                form: null,
                harness_runs_in: 'provider',
                held_by: 'provider',
                lifetime_secs: null,
                metering: 'wire',
                model_enforcement: 'wire',
                orch_tools: { offered: true, reason: null },
                tool_set: null,
                tools: 'own',
                type: 'separated',
              },
              // Codex and Claude Code declare plan mode; other programs do not.
              plan_mode: refs.agent.harness_kind === 'codex' || refs.agent.harness_kind === 'claude-code',
              plan_mechanics: refs.agent.harness_kind === 'codex' ? { proposal: 'item', proceed: 'message' } : refs.agent.harness_kind === 'claude-code' ? { proposal: 'request', proceed: 'allow' } : null,
              provider: refs.provider,
            },
      blocked: blockedFor(treeThread?.blocked_reason),
      can: { archive: base.access.permissions.includes('thread.archive') || base.access.permissions.includes('thread.manage'), export: base.access.permissions.includes('thread.export'), fork: true, place: base.access.permissions.includes('thread.manage') },
      current_segment_id: SEGMENT(row.id),
      current_turn_id: running?.id ?? null,
      cwd: '/work',
      directory: '',
      // No source is prepared and no link was accepted; the work in the directory has not been looked at, so its flags are unknown.
      source: null,
      links: [],
      suggested_links: [],
      work: null,
      attention: { uncommitted: null, unpushed: null, failed: turns.at(-1)?.state === 'failed', as_of: null, stale: false, reason: null },
      directory_threads: row.id === ACME.mainThread ? 1 : 0,
      effort: bound.effort,
      environment: refs.environment,
      environment_permissions: this.world.access.environmentPermissions(person, row.id),
      input_access: mayInteract
        ? { allowed: true, missing: [] }
        : { allowed: false, missing: [{ ask: [owner], ask_more: 0, override: false, closed: null, object: { id: row.id, kind: 'thread', name: row.name, owner }, options: null, permissions: ['thread.interact'] }] },
      last_activity_at: treeThread?.last_activity_at ?? null,
      lock: null,
      memory: { kept_by: 'agent', native: true, why: null },
      model: bound.model,
      model_display: displayOf(resources.modelsOf(bound.provider).find((entry) => entry.model_name === bound.model)?.display_name, bound.model),
      my_last_read_seq: held.read.get(person.me.username) ?? null,
      pending_approvals: this.pendingApprovals(row.id),
      pending_inputs: pendingInputs,
      queue: { held: false, by: null, at: null, held_inputs: 0, revision: 1 },
      seq: held.seq,
      side_from_turn: null,
      side_of: null,
      sides: { allowed: true, open: 0, max: 4, mode: 'read_only', reason: null },
      side_reading: null,
      starting_step: null,
      display: { ...(treeThread?.display ?? executionDisplay(treeThread?.state ?? 'idle')), seq: held.seq, read_seq: held.read.get(person.me.username) ?? 0 },
      state: treeThread?.state ?? 'idle',
      topic_id: row.parent_id ?? '',
      usage: { cached_input_tokens: 1000, compact_at: null, context_used: 42_000, context_window: 200_000, input_tokens: 12_000, output_tokens: 3_000, reasoning_tokens: 500, total_tokens: 15_000, updated_at: MADE_AT },
    };
    return { ...made, ...held.fields, display: { ...(held.fields.display ?? made.display), seq: held.seq, read_seq: held.read.get(person.me.username) ?? 0 } };
  }

  private pendingApprovals(threadId: string): number {
    return [...this.held(threadId).items.values()].filter((item) => item.kind === 'approval' && item.status === 'pending').length;
  }

  /** A thread as a node, for the requests that serve every node alike. */
  node(person: Person, row: TreeNode): Node {
    return this.thread(person, row);
  }

  private sync(person: Person, threadId: string, since: number | null, limit: number): ThreadSync {
    const row = this.rowOf(threadId);
    this.places.need(person, threadId, 'thread.view');
    const held = this.held(threadId);
    const all = [...held.items.values()].sort((one, other) => one.seq_created - other.seq_created);
    let items: Item[];
    let hasOlder = false;
    let turns: Turn[];
    let inputs: ThreadInput[];
    if (since === null) {
      const main = all.filter((item) => item.stream === 'main');
      items = [...new Map([...all.slice(-limit), ...main.slice(-limit)].map((item) => [item.id, item])).values()]
        .sort((one, other) => one.seq_created - other.seq_created);
      hasOlder = main.length > limit;
      const turnList = [...held.turns.values()].sort((one, other) => one.ordinal - other.ordinal);
      const over = turnList.filter((entry) => entry.state !== 'running' && entry.state !== 'starting').slice(-20);
      const needed = new Set(items.map((item) => item.turn_id));
      turns = [...new Map([...turnList.filter((entry) => entry.state === 'running' || entry.state === 'starting' || needed.has(entry.id)), ...over].map((turn) => [turn.id, turn])).values()];
      inputs = [...held.inputs.values()].filter((entry) => ['pending', 'delivering', 'failed'].includes(entry.state));
    } else {
      items = all.filter((item) => item.seq_updated > since);
      turns = [...held.turns.values()].filter((entry) => entry.seq_updated > since);
      inputs = [...held.inputs.values()].filter((entry) => entry.seq_updated > since);
    }
    return {
      complete: true,
      has_older: hasOlder,
      inputs,
      items: items.map((item) => this.itemFor(person, threadId, item)),
      segments: [...held.segments.values()].filter((segment) => since === null ? segment.id === this.thread(person, row).current_segment_id || items.some((item) => item.segment_id === segment.id) || turns.some((turn) => turn.segment_id === segment.id) : segment.seq_updated > since),
      seq: held.seq,
      since,
      thread: this.thread(person, row),
      turns,
    };
  }

  // ---------------------------------------------------------------- emitting rows

  /** Keeps rows in a thread and sends them to its followers as one frame; `lose` keeps them without sending. */
  emit(threadId: string, rows: Rows, lose = false): number {
    const row = this.rowOf(threadId);
    const held = this.held(threadId);
    const from = held.seq;
    // Every row that is made or changed takes the next sequence of its own, as the real system numbers them,
    // so that a thread read a page at a time by the sequence an item was made at loses none of them.
    const next = (): number => (held.seq += 1);
    const data: ThreadData = {};
    if (rows.segments !== undefined || held.segments.size === 0) {
      const segments = rows.segments ?? [this.firstSegment(threadId)];
      data.segments = segments.map((segment) => {
        const made = { ...segment, id: segment.id === '' ? SEGMENT(threadId) : segment.id, seq_updated: next() };
        held.segments.set(made.id, made);
        return made;
      });
    }
    if (rows.items !== undefined) {
      data.items = rows.items.map((item) => {
        const earlier = held.items.get(item.id);
        const at = next();
        const made = { ...item, thread_id: threadId, segment_id: item.segment_id === '' ? SEGMENT(threadId) : item.segment_id, seq_created: earlier?.seq_created ?? at, seq_updated: at, updated_at: new Date().toISOString() };
        held.items.set(made.id, made);
        return made;
      });
    }
    if (rows.turns !== undefined) {
      data.turns = rows.turns.map((turn) => {
        const made = { ...turn, thread_id: threadId, segment_id: turn.segment_id === '' ? SEGMENT(threadId) : turn.segment_id, seq_updated: next() };
        held.turns.set(made.id, made);
        return made;
      });
    }
    if (rows.inputs !== undefined) {
      data.inputs = rows.inputs.map((input) => {
        const made = { ...input, thread_id: threadId, seq_updated: next() };
        held.inputs.set(made.id, made);
        return made;
      });
    }
    // A frame that changes only the thread itself still moves the sequence on.
    if (held.seq === from) next();
    const seq = held.seq;
    if (rows.thread !== undefined) {
      if (rows.thread.display !== undefined) rows.thread.display = { ...rows.thread.display, seq };
      held.fields = { ...held.fields, ...rows.thread };
      // The tree row is what the left bar and the topic read: it follows the thread's state.
      const state = rows.thread.state ?? row.thread?.state ?? 'idle';
      this.world.changeTree(this.places.rootOf(threadId) ?? ACME.org, [
        { id: threadId, thread: { display: rows.thread.display ?? executionDisplay(state, { seq, blocked_reason: rows.thread.blocked?.reason ?? null }), state, attention: rows.thread.attention ?? row.thread?.attention ?? { uncommitted: null, unpushed: null, failed: false, as_of: null, stale: false, reason: null }, blocked_reason: rows.thread.blocked?.reason ?? null, pending_approvals: 0, activity_seq: seq, last_activity_at: new Date().toISOString() } },
      ]);
    }
    // The thread row goes with every frame, as the real server does when something in it changed.
    const meta = this.places.metaOf(threadId);
    meta.revision = Math.max(meta.revision, 1);
    for (const [subscriber, follower] of followers(held)) {
      const person = this.world.people.get(follower.user);
      if (person === undefined) continue;
      const own: ThreadData = { ...data, ...(data.items === undefined || data.items === null ? {} : { items: data.items.map((item) => this.itemFor(person, threadId, item)) }), thread: this.thread(person, this.rowOf(threadId)) };
      const frame = { op: 'sync', id: follower.id, feed: `thread:${threadId}`, prev: follower.last, seq, data: own };
      follower.last = seq;
      if (!lose) subscriber.send(frame);
    }
    return seq;
  }

  private firstSegment(threadId: string): Segment {
    const refs = this.world.resources.refsOf(this.world.resources.bindingOf(threadId));
    return {
      agent: refs.agent ?? ({} as AgentRef),
      arrangement: 'separated',
      credential_path: 'native',
      end_reason: null,
      ended_at: null,
      env_generation: 1,
      environment: refs.environment ?? ({} as EnvironmentRef),
      executor: null,
      id: SEGMENT(threadId),
      import_fidelity: null,
      import_report: null,
      ordinal: 1,
      provider: refs.provider ?? ({} as ProviderRef),
      seq_updated: 0,
      started_at: MADE_AT,
      started_by: null,
      tool_set: null,
    };
  }

  /** A piece of growing text, sent as it is, without being kept: it is the rows that keep it. */
  delta(threadId: string, piece: { item_id: string; field: string; offset: number; len: number; data: string }): void {
    for (const [subscriber, follower] of followers(this.held(threadId))) {
      subscriber.send({ op: 'delta', id: follower.id, feed: `thread:${threadId}`, ...piece });
    }
  }

  setPresence(threadId: string, users: PresenceUser[]): void {
    const held = this.held(threadId);
    held.presence = users;
    for (const [subscriber, follower] of followers(held)) subscriber.send({ op: 'presence', id: follower.id, feed: `thread:${threadId}`, users });
  }

  closeFeed(threadId: string, reason: 'revoked' | 'deleted'): void {
    const held = this.held(threadId);
    for (const [subscriber, follower] of followers(held)) subscriber.send({ op: 'closed', id: follower.id, feed: `thread:${threadId}`, reason });
    held.followers.clear();
  }

  // ---------------------------------------------------------------- the feed of a thread

  /** The frames of the live connection about a thread's feed: a subscription, the end of one, and whether the thread is on screen. */
  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    const id = Number(frame.id);
    const feed = typeof frame.feed === 'string' ? frame.feed : '';
    const threadId = /^thread:(.+)$/.exec(feed)?.[1];
    switch (frame.op) {
      case 'sub': {
        if (threadId === undefined) return false;
        if (person === undefined || this.places.rowOf(threadId)?.kind !== 'thread') {
          subscriber.send({ op: 'error', id, code: 'not_found', detail: feed });
          return true;
        }
        this.subscribe(person, threadId, subscriber, id, typeof frame.since === 'number' ? frame.since : null);
        return true;
      }
      case 'unsub': {
        for (const [heldId, held] of this.threads) {
          const own = held.followers.get(subscriber);
          if (own?.delete(id) !== true) continue;
          if (own.size === 0) held.followers.delete(subscriber);
          subscriber.send({ op: 'closed', id, feed: `thread:${heldId}`, reason: 'unsubscribed' });
          return true;
        }
        return false;
      }
      case 'visible':
        if (threadId === undefined) return false;
        this.setVisible(threadId, subscriber, frame.visible === true);
        return true;
      default:
        return false;
    }
  }

  /** A live connection ended: it follows no thread any more. */
  gone(subscriber: Subscriber): void {
    this.unsubscribe(subscriber);
  }

  /** A subscription to a thread's feed: the frames that answer it, sent through `subscriber`. */
  subscribe(person: Person, threadId: string, subscriber: Subscriber, id: number, since: number | null): boolean {
    const held = this.held(threadId);
    this.rowOf(threadId);
    if (!this.places.access(person, threadId).permissions.includes('thread.view')) {
      subscriber.send({ op: 'error', id, code: 'forbidden', detail: `thread:${threadId}` });
      return false;
    }
    const reload = since !== null && since > held.seq;
    let own = held.followers.get(subscriber);
    if (own === undefined) { own = new Map(); held.followers.set(subscriber, own); }
    own.set(id, { id, last: reload ? held.seq : (since ?? held.seq), user: person.me.username, visible: false });
    subscriber.send({ op: 'subscribed', id, feed: `thread:${threadId}`, seq: held.seq, mode: reload ? 'reload' : 'delta' });
    subscriber.send({ op: 'presence', id, feed: `thread:${threadId}`, users: held.presence });
    if (!reload && since !== null && since < held.seq) {
      const rows = this.sync(person, threadId, since, 500);
      const follower = held.followers.get(subscriber)?.get(id);
      subscriber.send({ op: 'sync', id, feed: `thread:${threadId}`, prev: since, seq: held.seq, data: { thread: rows.thread, items: rows.items, turns: rows.turns, inputs: rows.inputs, segments: rows.segments } });
      if (follower !== undefined) follower.last = held.seq;
    }
    return true;
  }

  /** Ends a subscription to a thread's feed, or all of a connection's when it closes. */
  unsubscribe(subscriber: Subscriber, threadId?: string): void {
    if (threadId !== undefined) {
      this.threads.get(threadId)?.followers.delete(subscriber);
      return;
    }
    for (const held of this.threads.values()) held.followers.delete(subscriber);
  }

  setVisible(threadId: string, subscriber: Subscriber, visible: boolean): void {
    const own = this.threads.get(threadId)?.followers.get(subscriber);
    for (const follower of own?.values() ?? []) follower.visible = visible;
  }

  /** An item of a thread, for the parts that speak of it (a forward copies it). */
  itemOf(threadId: string, itemId: string): Item | undefined {
    return this.threads.get(threadId)?.items.get(itemId);
  }

  /** Exact active work identities for the archive fence, without any input or conversation body. */
  archiveIdentity(threadId: string) {
    const held = this.held(threadId);
    return {
      turns:[...held.turns.values()].filter((turn) => turn.state === 'starting' || turn.state === 'running').map((turn) => [turn.id,turn.state,turn.seq_updated]).sort(),
      inputs:[...held.inputs.values()].filter((input) => input.state === 'pending' || input.state === 'delivering').map((input) => [input.id,input.state,input.seq_updated]).sort(),
      approvals:[...this.approvals.values()].filter((approval) => approval.thread_id === threadId && approval.state === 'pending').map((approval) => [approval.id,approval.state]).sort(),
      segments:[...held.segments.values()].map((segment) => [segment.id,segment.ended_at,segment.seq_updated]).sort(),links:held.fields.links ?? [],
    };
  }

  /** Counts the retained work an archive settles; these facts are cut by the place reader. */
  archiveFacts(threadId: string) {
    const held = this.held(threadId);
    return { turns:[...held.turns.values()].filter((turn) => turn.state === 'starting' || turn.state === 'running').length,
      inputs:[...held.inputs.values()].filter((input) => input.state === 'pending' || input.state === 'delivering').length,
      approvals:[...this.approvals.values()].filter((approval) => approval.thread_id === threadId && approval.state === 'pending').length,
      leases:0,session:[...held.segments.values()].some((segment) => segment.ended_at === null),links:held.fields.links?.length ?? 0 };
  }

  /** Settles the same retained rows the archive review counted, without starting or reviving work. */
  settleArchive(threadId: string, person: Person): void {
    const held = this.held(threadId);
    const now = new Date().toISOString();
    for (const [id,approval] of this.approvals) if (approval.thread_id === threadId && approval.state === 'pending') this.approvals.set(id,{...approval,state:'cancelled'});
    this.emit(threadId,{
      thread:{state:'idle',current_turn_id:null},
      turns:[...held.turns.values()].filter((turn) => turn.state === 'starting' || turn.state === 'running').map((turn): Turn => ({...turn,state:'interrupted',completed_at:now,phase:null,interrupted_by:userRef(person.me)})),
      inputs:[...held.inputs.values()].filter((input) => input.state === 'pending' || input.state === 'delivering').map((input): ThreadInput => ({...input,state:'cancelled'})),
      segments:[...held.segments.values()].map((segment): Segment => segment.ended_at === null ? {...segment,ended_at:now,end_reason:'archived'} : segment),
    });
  }

  /** What the fake holds of a thread, for a test to read: the inputs people queued, the drafts, the sequence. */
  snapshot(threadId: string): unknown {
    const held = this.held(threadId);
    return {
      seq: held.seq,
      items: [...held.items.values()],
      turns: [...held.turns.values()],
      inputs: [...held.inputs.values()],
      drafts: Object.fromEntries(held.drafts),
      read: Object.fromEntries(held.read),
      created: this.created,
    };
  }

  /** Who the fake was told is looking at each thread: for a test to read. */
  visibility(): Record<string, boolean[]> {
    const out: Record<string, boolean[]> = {};
    for (const [threadId, held] of this.threads) out[threadId] = [...followers(held)].map(([, follower]) => follower.visible);
    return out;
  }

  // ---------------------------------------------------------------- what a test sets

  configure(threadId: string, body: Record<string, unknown>): void {
    const held = this.held(threadId);
    if (typeof body.cannot_steer === 'boolean') held.cannotSteer = body.cannot_steer;
    if (body.limits !== undefined) held.limits = body.limits as ThreadLimits;
    if (body.default_delivery === 'queue' || body.default_delivery === 'steer' || body.default_delivery === 'interrupt') held.defaultDelivery = body.default_delivery;
    if (body.reset === true) {
      this.settingRows.delete(threadId);
      held.items.clear();
      held.turns.clear();
      held.inputs.clear();
      held.segments.clear();
      held.fields = {};
      held.drafts.clear();
      held.clientIds.clear();
      held.position = 0;
      held.read.clear();
      held.cannotSteer = false;
      held.limits = { bypass_all: false, reached: [] };
      held.defaultDelivery = 'queue';
      held.params = {};
      held.paramsRevision = 0;
      held.lockedBy = null;
    }
    if (body.lock !== undefined) held.lockedBy = body.lock === null ? null : (body.lock as ThreadChoices['lockedBy']);
    if (body.params !== undefined && typeof body.params === 'object' && body.params !== null) {
      held.params = { ...(body.params as Record<string, unknown>) };
      held.paramsRevision += 1;
    }
  }

  addApproval(approval: Approval): void {
    this.approvals.set(approval.id, approval);
  }

  /**
   * A model's entry at the provider the threads run on, written anew as a manager of the provider does: what it offers,
   * what it opens to threads and what a thread starts with. A model the provider does not have is added with the name it
   * is shown by.
   */
  setEntry(modelName: string, display: string, parameters: ProviderModel['parameters'], groups?: string[]): void {
    const provider = this.world.resources.bindingOf(THREAD_PROVIDER).provider;
    const models = this.world.resources.modelsOf(provider);
    const entry = models.find((candidate) => candidate.model_name === modelName);
    if (entry === undefined) {
      models.push({ ...MODEL_ESTIMATE, id: randomUUID(), model_name: modelName, display_name: display, enabled: true, source: 'manual', groups: groups ?? [], parameters });
      return;
    }
    entry.parameters = parameters;
    if (display !== '') entry.display_name = display;
    if (groups !== undefined) entry.groups = groups;
  }

  /** Forgets everything: a test begins from the team as it was. */
  reset(): void {
    for (const held of this.threads.values()) {
      for (const subscriber of held.followers.keys()) subscriber.close(1012);
    }
    this.threads.clear();
    this.settingRows.clear();
    this.approvals.clear();
    this.created = 0;
  }

  // ---------------------------------------------------------------- the requests

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;

    if ((found = /^POST \/topics\/([^/]+)\/threads$/.exec(route))) return { status: 200, body: this.makeThread(person, found[1] ?? '', body) };

    if ((found = /^GET \/approvals\/([^/]+)$/.exec(route))) {
      const approval = this.approvals.get(found[1] ?? '');
      // An approval that is not a thread's is another part's to answer (a request for access).
      if (approval === undefined) return null;
      return { status: 200, body: this.approvalFor(person, approval) };
    }
    if ((found = /^POST \/approvals\/([^/]+)\/decision$/.exec(route))) return this.approvals.has(found[1] ?? '') ? { status: 200, body: this.decide(person, found[1] ?? '', body) } : null;

    if ((found = /^GET \/threads\/([^/]+)\/sync$/.exec(route))) {
      const since = query.get('since');
      return { status: 200, body: this.sync(person, found[1] ?? '', since === null ? null : Number(since), Number(query.get('limit') ?? 200)) };
    }
    if ((found = /^GET \/threads\/([^/]+)\/(turns|segments)$/.exec(route))) return { status: 200, body: this.history(person, found[1] ?? '', found[2] === 'turns', query) };
    if ((found = /^GET \/threads\/([^/]+)\/items$/.exec(route))) return { status: 200, body: this.listItems(person, found[1] ?? '', query) };
    if ((found = /^GET \/threads\/([^/]+)\/items\/([^/]+)$/.exec(route))) {
      this.places.need(person, found[1] ?? '', 'thread.view');
      const item = this.held(found[1] ?? '').items.get(found[2] ?? '');
      if (item === undefined) throw new Refusal(404, 'not_found', 'there is no such item');
      return { status: 200, body: this.itemFor(person, found[1] ?? '', item) };
    }
    if ((found = /^GET \/threads\/([^/]+)\/items\/([^/]+)\/output$/.exec(route))) {
      this.places.need(person, found[1] ?? '', 'thread.view');
      const item = this.held(found[1] ?? '').items.get(found[2] ?? '');
      const whole = item?.kind === 'tool_call' && item.payload.tool.kind === 'command' ? (this.wholeOutput.get(item.id) ?? item.payload.tool.output.inline) : '';
      const from = Number(query.get('offset') ?? 0);
      const length = Number(query.get('limit') ?? 262_144);
      return { status: 200, text: whole.slice(from, from + length) };
    }
    if ((found = /^GET \/threads\/([^/]+)\/catchup$/.exec(route))) {
      const held = this.held(found[1] ?? '');
      return { status: 200, body: { since: Number(query.get('since') ?? 0), seq: held.seq, items: held.items.size, turns_completed: 3, turns_failed: 0, commands: 12, files_changed: 4, approvals_pending: 0, errors: 0, milestones: [], announcements: [] } };
    }
    if ((found = /^GET \/threads\/([^/]+)\/limits$/.exec(route))) {
      this.places.need(person, found[1] ?? '', 'thread.view');
      return { status: 200, body: this.world.usage.threadLimits(person, found[1] ?? '') ?? this.held(found[1] ?? '').limits };
    }
    if ((found = /^GET \/threads\/([^/]+)\/(context|usage\/turns)$/.exec(route))) {
      const threadId = found[1] ?? '';
      this.places.need(person, threadId, 'thread.view');
      const harness = this.thread(person, this.rowOf(threadId)).binding?.agent.harness_kind ?? null;
      const turns = this.held(threadId).turns.values();
      if (found[2] === 'context') return { status: 200, body: contextOf(turns, harness) };
      const before = query.get('before');
      return { status: 200, body: turnUsagePage(turns, harness, before === null ? null : Number(before), Number(query.get('limit') ?? 200)) };
    }
    if ((found = /^GET \/threads\/([^/]+)\/turns\/([^/]+)\/cost$/.exec(route))) {
      this.places.need(person, found[1] ?? '', 'thread.view');
      const turn = this.held(found[1] ?? '').turns.get(found[2] ?? '');
      if (turn === undefined) throw new Refusal(404, 'not_found', 'there is no such turn');
      return { status: 200, body: turnCostOf(turn) };
    }
    if ((found = /^GET \/threads\/([^/]+)\/parameters$/.exec(route))) return { status: 200, body: this.parameters(person, found[1] ?? '') };
    if ((found = /^POST \/threads\/([^/]+)\/inputs$/.exec(route))) return { status: 201, body: this.queue(person, found[1] ?? '', body) };
    if ((found = /^(PATCH|DELETE) \/threads\/([^/]+)\/inputs\/([^/]+)$/.exec(route))) {
      const threadId = found[2] ?? '', held = this.held(threadId), since = held.seq;
      const input = this.changeInput(person, threadId, found[3] ?? '', found[1] === 'DELETE', body);
      if (found[1] === 'DELETE') return { status: 200, body: input };
      const reply: ThreadInputUpdateReply = { input, inputs: this.places.access(person, threadId).permissions.includes('thread.view')
        ? [...held.inputs.values()].filter((row) => row.seq_updated > since) : [] };
      return { status: 200, body: reply };
    }
    if ((found = /^POST \/threads\/([^/]+)\/queue\/resume$/.exec(route))) {
      const threadId = found[1] ?? '';
      this.places.need(person, threadId, 'thread.interact');
      const held = this.held(threadId), queue = this.thread(person, this.rowOf(threadId)).queue;
      if (body.revision !== queue.revision) throw new Refusal(409, 'conflict.stale_revision', 'The waiting queue changed.');
      const resumed = { held: false, by: null, at: null, held_inputs: 0, revision: queue.revision + 1 };
      this.emit(threadId, { thread: { queue: resumed }, inputs: [...held.inputs.values()].filter((input) => input.queue_held && input.state === 'pending').map((input) => ({ ...input, queue_held: false })) });
      return { status: 200, body: resumed };
    }
    if ((found = /^POST \/threads\/([^/]+)\/interrupt$/.exec(route))) return { status: 200, body: this.interrupt(person, found[1] ?? '', body) };
    if ((found = /^POST \/threads\/([^/]+)\/turns\/([^/]+)\/retry$/.exec(route))) return { status: 201, body: this.retry(person, found[1] ?? '', found[2] ?? '', body) };
    if ((found = /^(PUT|DELETE) \/threads\/([^/]+)\/items\/([^/]+)\/pin$/.exec(route))) {
      const threadId = found[2] ?? '';
      this.places.need(person, threadId, 'thread.interact');
      const item = this.held(threadId).items.get(found[3] ?? '');
      if (item === undefined) throw new Refusal(404, 'not_found', 'there is no such item');
      const pinned = found[1] === 'PUT' ? { at: new Date().toISOString(), by: userRef(person.me) } : null;
      this.emit(threadId, { items: [{ ...item, pinned }] });
      return { status: 200, body: this.held(threadId).items.get(item.id) };
    }
    if ((found = /^GET \/threads\/([^/]+)\/draft$/.exec(route))) return { status: 200, body: this.held(found[1] ?? '').drafts.get(person.me.username) ?? null };
    if ((found = /^PUT \/threads\/([^/]+)\/draft$/.exec(route))) {
      const drafts = this.held(found[1] ?? '').drafts;
      // Comments left out of a save are the ones the author already has, as the server keeps them.
      const comments = (body.review_comments as Draft['review_comments'] | undefined) ?? drafts.get(person.me.username)?.review_comments ?? [];
      const nativeAction = Object.hasOwn(body, 'native_action') ? (body.native_action ?? null) as Draft['native_action'] : drafts.get(person.me.username)?.native_action ?? null;
      const draft: Draft = { content: (body.content ?? []) as Draft['content'], delivery: (body.delivery as Draft['delivery'] | undefined) ?? 'queue', native_action: nativeAction, review_comments: comments, updated_at: new Date().toISOString() };
      drafts.set(person.me.username, draft);
      return { status: 200, body: draft };
    }
    if ((found = /^DELETE \/threads\/([^/]+)\/draft$/.exec(route))) {
      this.held(found[1] ?? '').drafts.delete(person.me.username);
      return { status: 204 };
    }
    if ((found = /^POST \/threads\/([^/]+)\/read$/.exec(route))) {
      const held = this.held(found[1] ?? '');
      const seq = Math.max(Number(body.seq ?? 0), held.read.get(person.me.username) ?? 0);
      held.read.set(person.me.username, seq);
      // Every device of the person hears how far they have read, so that what is unread is the same on each.
      const threadId = found[1] ?? '';
      const current = this.thread(person, this.rowOf(threadId)).display;
      const icon = current.state === 'finished' && seq >= current.seq ? display('idle', { ...current, state: 'idle', read_seq: seq }) : current;
      held.fields.display = icon;
      this.world.users.tell(person.me.username, { what: 'read_state', thread_id: threadId, last_read_seq: seq, thread_display: icon });
      return { status: 200, body: { last_read_seq: seq, thread_display: icon } };
    }
    if (route === 'GET /me/read-states') {
      // How far the caller has read each thread they have a position in, all of them or those of one tree.
      const orgId = query.get('org_id');
      const states = [...this.threads.entries()].flatMap(([threadId, held]) => {
        const seq = held.read.get(person.me.username);
        return seq === undefined || (orgId !== null && this.places.rootOf(threadId) !== orgId) ? [] : [{ thread_id: threadId, last_read_seq: seq }];
      });
      return { status: 200, body: states };
    }
    if ((found = /^POST \/nodes\/([^/]+)\/auto$/.exec(route)) && this.places.rowOf(found[1] ?? '')?.kind === 'thread') {
      const threadId = found[1] ?? '';
      this.places.need(person, threadId, 'thread.manage');
      this.emit(threadId, { thread: { title_source: body.field === 'title' ? 'auto' : 'user', description_source: body.field === 'description' ? 'auto' : 'user' } });
      return { status: 200, body: this.thread(person, this.rowOf(threadId)) };
    }
    if ((found = /^PUT \/nodes\/([^/]+)\/settings\/([^/]+)$/.exec(route)) && this.places.rowOf(found[1] ?? '')?.kind === 'thread') {
      const threadId = found[1] ?? '';
      this.places.need(person, threadId, 'thread.manage');
      const key = found[2] ?? '';
      const value = body.value;
      const held = this.held(threadId);
      const current = this.settingsOf(threadId).find((setting) => setting.key === key);
      const revision = current?.own?.revision ?? 0;
      if (typeof body.expected_revision === 'number' && body.expected_revision !== revision) throw new Refusal(409, 'conflict.stale_revision', 'somebody changed this first', { current, changed_by: this.settingRows.get(threadId)?.get(key)?.by ?? null });
      if (key === 'agent.params' && held.lockedBy !== null && held.lockedBy !== undefined) throw new Refusal(403, 'access.locked', 'a place above locks the choices of this thread');
      const rows = this.settingRows.get(threadId) ?? new Map<string, { value: unknown; revision: number; by: Person['me'] }>();
      rows.set(key, { value, revision: revision + 1, by: person.me });
      this.settingRows.set(threadId, rows);
      if (key === 'agent.model' && typeof value === 'string') {
        const chosen = this.world.resources.modelsOf(this.world.resources.bindingOf(threadId).provider).find((entry) => entry.model_name === value);
        const display = chosen?.display_name ?? value;
        // A model that takes no effort leaves the thread with none.
        const takesEffort = (chosen?.parameters.efforts ?? []).length > 0;
        this.emit(threadId, { thread: { model: value, model_display: display, ...(takesEffort ? {} : { effort: null }) } });
      } else if (key === 'agent.effort') {
        this.emit(threadId, { thread: { effort: typeof value === 'string' ? value : null } });
      } else if (key === 'agent.params' && typeof value === 'object' && value !== null && !Array.isArray(value)) {
        held.params = { ...(value as Record<string, unknown>) };
        held.paramsRevision += 1;
        this.emit(threadId, {});
        return { status: 200, body: { key, value: held.params, source: { scope: 'thread', node: null }, own: { value: held.params, locked: false, revision: held.paramsRevision }, locked_by: null, writable: true, parts: [] } satisfies ResolvedSetting };
      }
      return { status: 200, body: this.settingsOf(threadId).find((setting) => setting.key === key) };
    }
    if ((found = /^DELETE \/nodes\/([^/]+)\/settings\/([^/]+)$/.exec(route)) && this.places.rowOf(found[1] ?? '')?.kind === 'thread') {
      const threadId = found[1] ?? '';
      this.places.need(person, threadId, 'thread.manage');
      const held = this.held(threadId);
      const key = found[2] ?? '';
      const current = this.settingsOf(threadId).find((setting) => setting.key === key);
      if (query.has('expected_revision') && Number(query.get('expected_revision')) !== current?.own?.revision) throw new Refusal(409, 'conflict.stale_revision', 'somebody changed this first', { current, changed_by: this.settingRows.get(threadId)?.get(key)?.by ?? null });
      if (held.lockedBy !== null && held.lockedBy !== undefined) throw new Refusal(403, 'access.locked', 'a place above locks the choices of this thread');
      this.settingRows.get(threadId)?.delete(key);
      if (key === 'agent.params') { held.params = {}; held.paramsRevision = 0; }
      if (key === 'agent.model') { delete held.fields.model; delete held.fields.model_display; }
      if (key === 'agent.effort') delete held.fields.effort;
      this.emit(threadId, {});
      return { status: 200, body: this.settingsOf(threadId).find((setting) => setting.key === key) ?? { key, value: null, source: { scope: 'default', node: null }, own: null, locked_by: null, writable: true, parts: [] } };
    }
    if ((found = /^GET \/nodes\/([^/]+)\/settings$/.exec(route))) {
      const kind = this.places.rowOf(found[1] ?? '')?.kind;
      // What a thread runs with is the thread's own; the settings of the places above it are the settings part's.
      if (kind === 'thread') return { status: 200, body: this.settingsOf(found[1] ?? '') };
    }
    // Whatever else is done to a thread as a node changes its row too, and whoever follows it is sent that.
    if ((found = /^(PATCH|POST|PUT|DELETE) \/nodes\/([^/]+)(?:\/.*)?$/.exec(route)) && this.places.rowOf(found[2] ?? '')?.kind === 'thread') {
      const answer = this.places.handle(person, method, path, query, body);
      if (answer !== null && answer.status < 300) this.emit(found[2] ?? '', {});
      return answer;
    }
    return null;
  }

  /** The whole output of commands whose output is longer than what is held inline, by item. */
  wholeOutput = new Map<string, string>();

  settingsOf(nodeId: string): ResolvedSetting[] {
    const held = this.held(nodeId);
    const setting = (key: string, value: unknown): ResolvedSetting => ({ key, value, source: { scope: 'default', node: null }, own: null, locked_by: null, writable: true, parts: [] });
    const params: ResolvedSetting =
      held.paramsRevision === 0 && Object.keys(held.params).length === 0
        ? setting('agent.params', {})
        : { key: 'agent.params', value: held.params, source: { scope: 'thread', node: null }, own: { value: held.params, locked: false, revision: held.paramsRevision }, locked_by: null, writable: true, parts: [] };
    const list = [setting('collab.default_delivery', held.defaultDelivery), setting('agent.model', null), setting('agent.effort', null), params];
    for (const [key, row] of this.settingRows.get(nodeId) ?? []) {
      if (key === 'agent.params') continue;
      const value: ResolvedSetting = { key, value: row.value, source: { scope: 'thread', node: null }, own: { value: row.value, locked: false, revision: row.revision }, locked_by: null, writable: true, parts: [] };
      const index = list.findIndex((setting) => setting.key === key);
      if (index === -1) list.push(value); else list[index] = value;
    }
    return list;
  }

  /** The same resource admission before a topic+thread act or a thread in an existing topic writes anything. */
  startAdmission(person: Person, topicId: string, body: Record<string, unknown>) {
    const resources = this.world.resources;
    const binding = body.binding as { agent_id?: string; provider_id?: string; arrangement?: ArrangementType | null } | undefined;
    const providerId = binding?.provider_id ?? '';
    if (!resources.readyFor(person, providerId)) {
      throw new Refusal(409, 'binding.incompatible', 'the provider cannot be used', {});
    }
    // A profile whose harness has not said what it is cannot be placed.
    if (!resources.agentReported(binding?.agent_id ?? '')) throw new Refusal(409, 'binding.incompatible', 'the harness of the profile has not reported what it is', { reason: 'harness_unreported' });
    const location = isRecord(body.environment) ? body.environment : {};
    const environmentId = typeof location.use === 'string' ? location.use : null;
    const fresh = isRecord(location.new) ? location.new : null;
    if (environmentId === null && fresh === null) throw new Refusal(400, 'environment.none_usable', 'name an existing or new environment');
    if (environmentId !== null && fresh !== null) throw new Refusal(400, 'request.invalid', 'choose one environment target');
    if (environmentId !== null && !resources.hasEnvironment(environmentId)) throw new Refusal(404, 'not_found', 'there is no such environment');
    const model = typeof body.model === 'string' ? body.model : '';
    if (!resources.modelsOf(providerId).some((entry) => entry.model_name === model && entry.enabled)) {
      throw new Refusal(409, 'model.not_allowed', `the model ${model} is not allowed`, {});
    }
    // The arrangement that was named has to be had and allowed, or nothing is made; with none named the first that can be had is taken.
    const arrangement = this.world.starts.arrangementFor(topicId, binding?.agent_id ?? '', providerId, environmentId, binding?.arrangement ?? null);
    return { resources, binding, environmentId, fresh, model, arrangement, providerId };
  }

  private makeThread(person: Person, topicId: string, body: Record<string, unknown>): Thread {
    this.places.need(person, topicId, 'topic.interact');
    const { resources, binding, fresh, model, arrangement, providerId, ...target } = this.startAdmission(person, topicId, body);
    let { environmentId } = target;
    if (fresh !== null) {
      const topicName = this.places.rowOf(topicId)?.name ?? 'New topic';
      const candidates = ['Main', topicName, ...Array.from({ length: 3 }, (_, index) => `${topicName} ${index + 2}`)];
      const name = fresh.name ?? candidates.find((candidate) => !resources.environments.some((environment) => environment.owner === person.me.username && environment.name === candidate));
      if (name === undefined) throw new Refusal(400, 'request.invalid', 'choose an unused environment name', { errors: [{ path: 'environment.new.name', message: 'choose an unused environment name' }] });
      const made = resources.handle(person, 'POST', '/environments', new URLSearchParams(), { ...fresh, name, home_node_id: fresh.home_node_id ?? topicId });
      if (!isRecord(made?.body) || typeof made.body.id !== 'string') throw new Error('the admitted environment has no identity');
      environmentId = made.body.id;
    }
    if (environmentId === null) throw new Error('the admitted thread has no environment');
    const row = this.places.makeThreadRow(person, topicId, typeof body.name === 'string' && body.name.trim() !== '' ? body.name.trim() : 'New thread');
    this.created += 1;
    resources.bind(row.id, {
      provider: providerId,
      agent: binding?.agent_id ?? '',
      environment: environmentId,
      model,
      effort: typeof body.effort === 'string' ? body.effort : null,
      directory: typeof body.directory === 'string' ? body.directory : '',
      arrangement,
    });
    if (body.access === 'private') this.world.access.apply(person, this.world.access.pageFor(row.id), { whole: 'private' });
    else if (body.access !== null && typeof body.access === 'object') this.world.access.apply(person, this.world.access.pageFor(row.id), body.access);
    // What the person said at once waits as the first input of the thread.
    const first = body.first_input as { content?: unknown; client_id?: string } | undefined;
    if (first?.content !== undefined) this.queue(person, row.id, { content: first.content, delivery: 'queue', client_id: first.client_id });
    return this.thread(person, this.rowOf(row.id));
  }

  private history(person: Person, threadId: string, turns: boolean, query: URLSearchParams): unknown {
    this.places.need(person, threadId, 'thread.view');
    const held = this.held(threadId);
    const before = query.get('before');
    const limit = Number(query.get('limit') ?? 100);
    if (!Number.isInteger(limit) || limit < 1 || limit > 500 || (before !== null && !/^[1-9]\d*$/.test(before))) throw new Refusal(400, 'request.invalid', 'invalid history page');
    const all = (turns ? [...held.turns.values()] : [...held.segments.values()]).filter((row) => before === null || row.ordinal < Number(before)).sort((a, b) => a.ordinal - b.ordinal);
    const rows = all.slice(-limit);
    const page = { next_before: all.length > limit ? rows[0]?.ordinal ?? null : null, seq: held.seq };
    return turns ? { ...page, turns: rows, segments: [...held.segments.values()].filter((segment) => (rows as Turn[]).some((turn) => turn.segment_id === segment.id)) } : { ...page, segments: rows };
  }

  private listItems(person: Person, threadId: string, query: URLSearchParams): unknown {
    this.places.need(person, threadId, 'thread.view');
    const held = this.held(threadId);
    const limit = Number(query.get('limit') ?? 100);
    const stream = query.get('stream');
    const all = [...held.items.values()].filter((item) => stream === null || item.stream === stream).sort((one, other) => one.seq_created - other.seq_created);
    const before = query.get('before');
    const after = query.get('after');
    const around = query.get('around');
    const ordinal = query.get('turn');
    let items: Item[];
    if (ordinal !== null) {
      if ([before,after,around].some((value) => value !== null) || !/^[1-9]\d*$/.test(ordinal)) throw new Refusal(400,'request.invalid','choose one numbered turn');
      const turn = [...held.turns.values()].find((turn) => turn.ordinal === Number(ordinal));
      const start = all.findIndex((item) => item.turn_id === turn?.id);
      if (turn === undefined || start < 0) throw new Refusal(404,'not_found','there is no such turn');
      items = all.slice(start,start+limit);
    }
    else if (before !== null) items = all.filter((item) => item.seq_created < Number(before)).slice(-limit);
    else if (after !== null) items = all.filter((item) => item.seq_created > Number(after)).slice(0, limit);
    else {
      const at = all.findIndex((item) => item.id === around);
      const from = Math.max(0, at - Math.floor(limit / 2));
      items = all.slice(from, from + limit);
    }
    const first = items[0];
    const last = items.at(-1);
    return {
      items: items.map((item) => this.itemFor(person, threadId, item)),
      has_older: first === undefined ? after !== null && all.some((item) => item.seq_created <= Number(after)) : all.some((item) => item.seq_created < first.seq_created),
      has_newer: last === undefined ? before !== null && all.some((item) => item.seq_created >= Number(before)) : all.some((item) => item.seq_created > last.seq_created),
      turns: [...held.turns.values()].filter((turn) => items.some((item) => item.turn_id === turn.id)),
      segments: [...held.segments.values()].filter((segment) => items.some((item) => item.segment_id === segment.id)),
    };
  }

  private parameters(person: Person, threadId: string): unknown {
    const row = this.rowOf(threadId);
    const thread = this.thread(person, row);
    const providerId = thread.binding?.provider.id ?? '';
    const options = this.world.resources.modelsOf(providerId).map((entry) => ({ ...MODEL_ESTIMATE, allowed: entry.enabled, display_name: entry.display_name, model_name: entry.model_name, ...(entry.groups.length === 0 ? {} : { groups: entry.groups }), parameters: entry.parameters, reason: entry.enabled ? null : 'disabled' }));
    const writable = this.places.access(person, threadId).permissions.includes('thread.manage');
    const source = { scope: 'default', node: null };
    const held = this.held(threadId);
    // Harness declarations and their stored choices use the same owner as model parameters.
    const parameters = this.world.resources
      .declaredBy(thread.binding?.agent.id ?? '')
      .filter((def): def is Record<string, unknown> & { key: string } => typeof def.key === 'string' && Array.isArray(def.scopes) && def.scopes.includes('thread'))
      .map((def) => {
        const own = Object.hasOwn(held.params, def.key);
        return { apply: def.apply, def, key: def.key, locked_by: held.lockedBy ?? null,
          source: own ? { scope: 'thread', node: null } : source, value: own ? held.params[def.key] : null,
          writable: writable && (held.lockedBy === null || held.lockedBy === undefined) };
      });
    return {
      provider: providerId === '' ? undefined : this.world.resources.providerForFacts(person, providerId),
      effort: { source, supported: true, value: thread.effort, writable },
      model: { options, source, value: thread.model, writable },
      parameters: [...parameters, ...this.parameterViewsOf(person, threadId, thread.model)],
      approval_mode: this.world.settings.valueAt(threadId, 'approval.mode').value,
      approval_modes: ['ask', 'on_request', ...(thread.environment?.placement === 'managed' ? ['bypass'] : [])]
        .filter((mode) => { const allowed = this.world.settings.valueAt(threadId, 'approval.allowed_modes').value; return Array.isArray(allowed) && allowed.includes(mode); }),
      approval_reviewer: null,
    };
  }

  /** The parameters of the thread's agent that exist for its model, with the thread's own choices. */
  private parameterViewsOf(person: Person, threadId: string, model: string | null): ParameterView[] {
    const held = this.held(threadId);
    const row = this.rowOf(threadId);
    const providerId = this.thread(person, row).binding?.provider.id ?? '';
    const entry = this.world.resources.modelsOf(providerId).find((candidate) => candidate.model_name === model);
    if (entry === undefined) return [];
    const writable = this.places.access(person, threadId).permissions.includes('thread.manage');
    return parameterViews(entry.parameters, { own: held.params, features: this.features, writable, lockedBy: held.lockedBy });
  }

  /** The normal queued-input preparation, also read by the people preview without publishing a row. */
  prepareInput(person: Person, threadId: string, body: Record<string, unknown>): ThreadInput {
    this.places.need(person, threadId, 'thread.interact');
    const held = this.held(threadId);
    const clientId = typeof body.client_id === 'string' ? body.client_id : randomUUID();
    const seen = held.clientIds.get(`${person.me.username}:${clientId}`);
    if (seen !== undefined) return must(held.inputs.get(seen), 'input');
    const delivery = (body.delivery as ThreadInput['delivery'] | undefined) ?? 'queue';
    if (delivery === 'steer' && held.cannotSteer) throw new Refusal(409, 'capability.unsupported', 'this agent cannot be steered');
    const input: ThreadInput = {
      native_action: (body.native_action ?? null) as ThreadInput['native_action'],
      author: userRef(person.me),
      client_id: clientId,
      content: (body.content ?? []) as ThreadInput['content'],
      created_at: new Date().toISOString(),
      delivered_at: null,
      delivery,
      id: randomUUID(),
      attachment_issues: [],
      queue_held: false,
      limit_bypass: [],
      plan: body.plan === true,
      position: String(held.position + 1),
      run_as: null,
      seq_updated: 0,
      source: 'user',
      state: 'pending',
      state_detail: null,
      thread_id: threadId,
      turn_id: null,
    };
    return input;
  }

  /** Accepts a checked input in this synchronous world's ordinary producer. */
  acceptInput(person: Person, input: ThreadInput, bypass?: unknown): { input: ThreadInput; seq: number } {
    const threadId = input.thread_id;
    const held = this.held(threadId);
    const key = `${person.me.username}:${input.client_id ?? ''}`;
    const seen = held.clientIds.get(key);
    if (seen !== undefined) return {input:must(held.inputs.get(seen),'input'),seq:held.seq};
    this.world.usage.lettingPast(person, bypass);
    held.position += 1;
    input.position = String(held.position);
    held.clientIds.set(key, input.id);
    const seq = this.emit(threadId, { inputs: [input] });
    return { input: must(held.inputs.get(input.id), 'input'), seq };
  }

  private queue(person: Person, threadId: string, body: Record<string, unknown>): { input: ThreadInput; seq: number } {
    return this.acceptInput(person, this.prepareInput(person, threadId, body), body.bypass);
  }

  private changeInput(person: Person, threadId: string, inputId: string, cancel: boolean, body: Record<string, unknown>): ThreadInput {
    this.places.need(person, threadId, 'thread.interact');
    const held = this.held(threadId);
    const input = held.inputs.get(inputId);
    if (input === undefined) throw new Refusal(404, 'not_found', 'there is no such input');
    if (input.state !== 'pending') throw new Refusal(409, 'conflict.stale_revision', 'the input is no longer waiting', {});
    const publish = (next: ThreadInput) => {
      const queue = this.thread(person, this.rowOf(threadId)).queue;
      const waiting = [...held.inputs.values()].map((entry) => entry.id === next.id ? next : entry);
      // Native moves compact all pending positions, not just the picked row's number.
      const changed = !cancel && ('after_input_id' in body || 'delivery' in body)
        ? waiting.filter((row) => row.state === 'pending').sort((one, other) => Number(one.position) - Number(other.position))
          .map((row, index) => ({ ...row, position: String(index + 1) }))
          .filter((row) => row.id === next.id || row.position !== held.inputs.get(row.id)?.position)
        : [next];
      this.emit(threadId, { inputs: changed, thread: { queue: { ...queue, revision: queue.revision + 1, held_inputs: waiting.filter((entry) => entry.state === 'pending' && entry.queue_held).length } } });
      return must(held.inputs.get(inputId), 'input');
    };
    if (cancel) {
      return publish({ ...input, state: 'cancelled' });
    }
    const next: ThreadInput = { ...input };
    if ('delivery' in body) {
      if (body.delivery !== 'queue' && body.delivery !== 'steer' && body.delivery !== 'interrupt') throw new Refusal(400, 'request.invalid', 'choose a delivery');
      if (body.delivery === 'steer' && held.cannotSteer && [...held.turns.values()].some((turn) => turn.state === 'running')) throw new Refusal(409, 'capability.unsupported', 'this agent cannot be steered', { capability: 'steer', fallback_delivery: 'interrupt' });
      next.delivery = body.delivery;
      if ('after_input_id' in body && (body.delivery === 'queue' || body.after_input_id !== null)) throw new Refusal(400, 'request.invalid', 'delivery and position conflict');
      const positions = [...held.inputs.values()].filter((entry) => entry.id !== inputId && entry.state === 'pending').map((entry) => Number(entry.position));
      next.position = String(body.delivery === 'queue' ? Math.max(0, ...positions) + 1 : Math.min(1, ...positions) - 1);
    }
    if (Array.isArray(body.content)) {
      if (input.author?.id !== person.me.id) throw new Refusal(403, 'authz.denied', 'only the author changes the words', { required: 'thread.interact', object: { kind: 'thread', id: threadId }, requestable: false, override: false, missing: [] });
      next.content = body.content as ThreadInput['content'];
    }
    if ('after_input_id' in body) {
      const others = [...held.inputs.values()].filter((entry) => entry.id !== inputId && entry.state === 'pending').sort((one, other) => Number(one.position) - Number(other.position));
      const after = body.after_input_id === null ? null : others.find((entry) => entry.id === body.after_input_id);
      if (after === undefined) throw new Refusal(409, 'input.not_pending', 'the input to follow is no longer waiting');
      const index = after === null ? -1 : others.indexOf(after);
      const before = after === null ? 0 : Number(after.position);
      const following = others[index + 1];
      next.position = String(after === null ? following === undefined ? 1 : Number(following.position) - 1 : following === undefined ? before + 1 : (before + Number(following.position)) / 2);
    }
    return publish(next);
  }

  private interrupt(person: Person, threadId: string, body: Record<string, unknown>): Turn {
    this.places.need(person, threadId, 'thread.interact');
    const held = this.held(threadId);
    const turn = held.turns.get(typeof body.turn_id === 'string' ? body.turn_id : '');
    if (turn === undefined || (turn.state !== 'running' && turn.state !== 'starting')) throw new Refusal(409, 'thread.not_running', 'that turn is not running');
    this.emit(threadId, { turns: [{ ...turn, state: 'interrupted', completed_at: new Date().toISOString(), phase: null, interrupted_by: userRef(person.me) }], thread: { state: 'idle', current_turn_id: null } });
    return must(held.turns.get(turn.id), 'turn');
  }

  private retry(person: Person, threadId: string, turnId: string, body: Record<string, unknown>): { input: ThreadInput; seq: number } {
    const held = this.held(threadId);
    const turn = held.turns.get(turnId);
    if (turn === undefined || (turn.state !== 'failed' && turn.state !== 'interrupted')) throw new Refusal(400, 'request.invalid', 'only a turn that failed or was stopped is tried again');
    const original = held.inputs.get(turn.input_id);
    return this.queue(person, threadId, { content: original?.content ?? [], delivery: 'queue', client_id: body.client_id });
  }

  private approvalFor(person: Person, approval: Approval): Approval {
    const permissions = this.places.access(person, approval.thread_id ?? '').permissions;
    return { ...approval, can_decide: approval.state === 'pending' && permissions.includes('thread.manage') && approval.can_decide };
  }

  /** Timeline snapshots belong to the current reader and are never kept in the shared item map. */
  private itemFor(person: Person, threadId: string, item: Item): Item {
    if (item.kind !== 'approval') return item;
    const approval = this.approvals.get(item.payload.approval_id);
    return { ...item, payload: { approval_id: item.payload.approval_id,
      ...(approval?.kind === 'harness' && approval.thread_id === threadId ? { approval: this.approvalFor(person, approval) } : {}),
    } };
  }

  private decide(person: Person, id: string, body: Record<string, unknown>): Approval {
    const approval = this.approvals.get(id);
    if (approval === undefined) throw new Refusal(404, 'not_found', 'there is no such approval');
    const threadId = approval.thread_id ?? '';
    this.places.need(person, threadId, 'thread.manage');
    if (approval.state !== 'pending') {
      throw new Refusal(409, 'approval.already_decided', 'somebody answered first', { current: this.approvalFor(person, approval) });
    }
    let state: Approval['state'] = 'approved';
    if (approval.kind === 'harness' && typeof body.option_id === 'string') {
      const option = approval.request.options.find((entry) => entry.id === body.option_id);
      if (option === undefined) throw new Refusal(400, 'request.invalid', 'there is no such option', { errors: [{ path: 'option_id', message: 'is not one of the options' }] });
      state = option.style === 'deny' ? 'denied' : 'approved';
    } else if (body.approve === false || body.action === 'decline' || body.action === 'cancel') {
      state = 'denied';
    } else if (approval.kind === 'harness' && (approval.request.request_kind === 'user_input' && typeof body.answers === 'object' && body.answers !== null || approval.request.request_kind === 'mcp_elicitation' && body.action === 'accept')) {
      state = 'answered';
    }
    const decided: Approval = { ...approval, state, decision: body, decided_at: new Date().toISOString(), decided_by: userRef(person.me) };
    this.approvals.set(id, decided);
    const held = this.held(threadId);
    const item = [...held.items.values()].find((entry) => entry.kind === 'approval' && entry.payload.approval_id === id);
    this.emit(threadId, { items: item === undefined ? [] : [{ ...item, status: state }], thread: { state: state === 'denied' ? 'idle' : 'running' } });
    // What waits for people has changed: everybody who is connected is told.
    this.world.notifications.settled();
    return this.approvalFor(person, decided);
  }
}
