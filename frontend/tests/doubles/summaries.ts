/** UI transport state only; real native authority, source and cost cases live in the Rust API/engine. */
import { randomUUID } from 'node:crypto';
import type { Chat, SummaryBatchReply, SummaryReceipt, SummarySnapshot, SummaryTarget } from '../../src/generated/api-types.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

function targetOf(kind: string, id: string): SummaryTarget {
  switch (kind) {
    case 'thread': return { kind: 'thread', id };
    case 'topic': return { kind: 'topic', id };
    case 'document': return { kind: 'document', id };
    case 'chat': return { kind: 'chat', id };
    case 'chat_title': return { kind: 'chat_title', id };
    default: throw new Refusal(404, 'not_found', 'Unknown summary target');
  }
}
function inputFacts(value: unknown): SummarySnapshot['input'] {
  if (typeof value !== 'object' || value === null || !('included_items' in value) || !('shortened_items' in value) || !('omitted_items' in value)) return null;
  const { included_items, shortened_items, omitted_items } = value;
  if (typeof included_items !== 'number' || typeof shortened_items !== 'number' || typeof omitted_items !== 'number'
    || ![included_items, shortened_items, omitted_items].every((count) => Number.isSafeInteger(count) && count >= 0) || shortened_items > included_items) return null;
  return { included_items, shortened_items, omitted_items };
}
function waitOf(value: unknown): SummarySnapshot['wait'] {
  if (typeof value !== 'object' || value === null || !('reason' in value) || !('until' in value)) return null;
  const { reason, until } = value;
  if (reason !== 'interval' && reason !== 'recovery' && reason !== 'queue') return null;
  if (until !== null && (typeof until !== 'string' || !Number.isFinite(Date.parse(until)))) return null;
  return { reason, until };
}
export class Summaries implements Part {
  readonly world: World;
  readonly rows = new Map<string, SummarySnapshot>();
  readonly requests: { user: string; target: SummaryTarget; body: Record<string, unknown> }[] = [];
  private readonly pending = new Set<string>();
  private readonly receipts = new Map<string, { body: string; receipt: SummaryReceipt }>();
  constructor(world: World) { this.world = world; }
  private key(user: string, target: SummaryTarget): string { return `${user}:${target.kind}:${target.id}`; }
  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path !== '/set') return null;
    const user = typeof body.user === 'string' ? body.user : 'ada';
    const kind = typeof body.kind === 'string' ? body.kind : 'document';
    const id = typeof body.id === 'string' ? body.id : '';
    const target = targetOf(kind, id);
    const key = this.key(user, target);
    const old = this.rows.get(key);
    const state = body.state;
    const states = ['empty', 'fresh', 'stale', 'scheduled', 'updating', 'failed', 'no_summarizer'] as const;
    const selected = states.find((value) => value === state) ?? 'fresh';
    const root = typeof body.root === 'string' ? body.root : this.world.places.rootOf(id) ?? '';
    this.rows.set(key, { root_org_id: root, target, inspecting: body.inspecting === true, revision: (old?.revision ?? 0) + 1, state: selected,
      text: typeof body.text === 'string' ? body.text : null, producer: (body.text === null || body.text === undefined) ? null : { kind: 'model', title: 'Model summarizer', provider: null, offering: null },
      updated_at: (body.text === null || body.text === undefined) ? null : new Date().toISOString(), route: null, input: inputFacts(body.input), manual_problem: body.manual_problem === 'cap_reached' || body.manual_problem === 'no_model' ? body.manual_problem : null, wait: waitOf(body.wait), poll_after_ms: typeof body.poll_after_ms === 'number' && body.poll_after_ms > 0 ? body.poll_after_ms : selected === 'updating' ? 5000 : null, problem: null });
    return { status: 204 };
  }
  opening(person: Person, chat: Chat): { summaries: SummarySnapshot[]; summary_range: { after: number; through: number } } {
    return { summaries: [this.read(person, { kind: 'chat', id: chat.id }), this.read(person, { kind: 'chat_title', id: chat.id })],
      summary_range: { after: Math.max(0, Math.min(chat.my?.last_read_seq ?? 0, chat.last_message_seq)), through: chat.last_message_seq } };
  }
  private current(person: Person, target: SummaryTarget): SummarySnapshot {
    let root: string | null;
    let inspecting = false;
    if (target.kind === 'document') {
      this.world.documents.handle(person, 'GET', `/documents/${target.id}`, new URLSearchParams(), {});
      const doc = this.world.documents.docs.get(target.id);
      root = doc === undefined ? null : this.world.places.rootOf(doc.nodeId);
    } else if (target.kind === 'chat' || target.kind === 'chat_title') {
      const opened = this.world.chats.handle(person, 'GET', `/chats/${target.id}`, new URLSearchParams(), {});
      if (typeof opened?.body === 'object' && opened.body !== null && 'can' in opened.body) {
        const can = opened.body.can;
        inspecting = typeof can === 'object' && can !== null && 'reason' in can && can.reason === 'inspecting';
      }
      root = this.world.chats.records.get(target.id)?.orgId ?? null;
    } else {
      if (target.kind === 'thread') this.world.threads.handle(person, 'GET', `/threads/${target.id}`, new URLSearchParams(), {});
      else this.world.places.handle(person, 'GET', `/topics/${target.id}`, new URLSearchParams(), {});
      root = this.world.places.rootOf(target.id);
    }
    if (root === null) throw new Refusal(404, 'not_found', 'No current summary source');
    const key = this.key(person.me.username, target);
    const stored = this.rows.get(key) ?? { root_org_id: root, target, inspecting: false, revision: 0, state: 'no_summarizer', text: null, producer: null, updated_at: null, route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null } satisfies SummarySnapshot;
    const held = { ...stored, inspecting: inspecting || stored.inspecting };
    return held;
  }
  private read(person: Person, target: SummaryTarget): SummarySnapshot {
    const key = this.key(person.me.username, target), held = this.current(person, target);
      if (this.pending.delete(key)) {
        this.rows.set(key, { ...held, revision: held.revision + 1, state: 'fresh', wait: null, poll_after_ms: null, text: `Completed summary for ${target.kind}.`, producer: { kind: 'model', title: 'Model summarizer', provider: null, offering: null }, updated_at: new Date().toISOString() });
      }
    return structuredClone(held);
  }
  handle(person: Person, method: string, path: string, _query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    if (method === 'POST' && path === '/summaries/batch') {
      if (!Array.isArray(body.targets)) throw new Refusal(422, 'request.invalid', 'Targets must be an array');
      const requested: SummaryTarget[] = body.targets.map((value: unknown) => {
        if (typeof value !== 'object' || value === null || !('kind' in value) || !('id' in value) || typeof value.kind !== 'string' || typeof value.id !== 'string') throw new Refusal(422, 'request.invalid', 'A target must name its native kind and id');
        return targetOf(value.kind, value.id);
      });
      const targets = [...new Map(requested.map((target) => [`${target.kind}:${target.id}`, target])).values()];
      const start = typeof body.offset === 'number' && Number.isSafeInteger(body.offset) && body.offset >= 0 ? body.offset : 0, end = Math.min(targets.length, start + 100);
      const reply: SummaryBatchReply = { items: [], unavailable: [], next_offset: end < targets.length ? end : null };
      for (const target of targets.slice(start, end)) {
        try {
          // Reuse only this UI transport fixture's existing source branch, not a native HTTP admission.
          const snapshot = this.read(person, target);
          const item = { target, root_org_id: snapshot.root_org_id, text: snapshot.text, producer: snapshot.producer, inspecting: snapshot.inspecting, updated_at: snapshot.updated_at,
            ...(body.full === true ? { full: snapshot } : {}) };
          reply.items.push(item);
        } catch (error) { if (error instanceof Refusal && (error.answer.status === 403 || error.answer.status === 404)) reply.unavailable.push(target); else throw error; }
      }
      return { status: 200, body: reply };
    }
    const match = /^\/summaries\/([^/]+)\/([^/]+)(\/refresh)?$/.exec(path);
    if (match === null) return null;
    const target = targetOf(match[1] ?? '', match[2] ?? '');
    if (method === 'GET' && match[3] === undefined) return { status: 200, body: this.read(person, target) };
    const key = this.key(person.me.username, target);
    const held = this.current(person, target);
    if (method === 'POST' && match[3] === '/refresh') {
      if (held.inspecting) throw new Refusal(403, 'authz.denied', 'The source is available for passive inspection only');
      this.requests.push({ user: person.me.username, target, body: structuredClone(body) });
      const identity = `${key}:${String(body.client_id)}`;
      const before = this.receipts.get(identity);
      if (before !== undefined) {
        if (before.body !== JSON.stringify(body)) throw new Refusal(409, 'conflict.exists', 'The refresh identity names another act');
        return { status: 202, body: structuredClone(before.receipt) };
      }
      const receipt: SummaryReceipt = { projection: 'receipt', id: randomUUID(), target, revision: held.revision + 1 };
      this.receipts.set(identity, { body: JSON.stringify(body), receipt });
      this.rows.set(key, { ...held, revision: held.revision + 1, state: 'updating', wait: { reason: 'queue', until: null }, poll_after_ms: 5000 }); this.pending.add(key);
      return { status: 202, body: receipt };
    }
    return null;
  }
}
