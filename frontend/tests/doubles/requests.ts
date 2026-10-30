/** What the fake API says of asking for access: the request that asks for what is missing, one approval for each thing, who is asked, how many may wait, and the decision that gives all, a part of it, or nothing.
 *
 * It keeps to what the real API promises as far as a screen can tell: a batch of up to eight items, each
 * an approval of kind access sent to those who manage the page and hold what is asked; a person has only so many
 * requests waiting; a decision is the approver's own change of the page, so that whoever decides can give only
 * what they hold; fewer permissions than were asked may be given, with an end and a note; a request is
 * complete when everything asked is held now, however that came about.
 */
import { randomUUID } from 'node:crypto';

import type { Approval, NodeRef, UserRef } from '../../src/generated/api-types.ts';
import type { Access } from './access.ts';
import { type Answer, type Person, Refusal } from './world.ts';

/** How many requests one person may have waiting at once (`access.max_pending_requests`). */
const WAITING_MAX = 10;
/** How many things one request may ask for. */
const ITEMS_MAX = 8;

export interface Stored {
  approval: Approval;
  requester: Person;
  /** The selected person, when the request is made for somebody else. */
  recipient?: Person;
  object: { kind: string; id: string; name: string };
  /** Where the page of the thing is asked for: the nodes', or the path of a kind of resource. */
  path: string;
  asked: string[];
  /** An approval a test put into the world: who may decide it, and who could with "everything I could decide", as it says, instead of what the page of the thing says. */
  told?: { deciders: Set<string>; also: Set<string> };
}

export class Requests {
  private records: Stored[] = [];
  private limit = WAITING_MAX;
  private readonly access: Access;

  constructor(access: Access) {
    this.access = access;
  }

  reset(): void {
    this.records = [];
    this.limit = WAITING_MAX;
  }

  private nodeRef(id: string): NodeRef {
    const found = this.access.nodeRefOf(id);
    return typeof found === 'string' ? { id, kind: 'org', name: null, redacted: true } : found;
  }

  /** Whether the person may decide it: they manage the page and hold what is asked. */
  mayDecide(person: Person, record: Stored): boolean {
    if (record.told !== undefined) return record.approval.state === 'pending' && record.told.deciders.has(person.me.username);
    const here = this.access.holdingOn(person, record.object.id, record.path);
    return record.approval.state === 'pending' && here.manages && record.asked.every((name) => here.held.has(name));
  }

  /** Whether the person could decide it, as the list of everything they could decide counts. */
  couldDecide(person: Person, record: Stored): boolean {
    return this.mayDecide(person, record) || record.told?.also.has(person.me.username) === true;
  }

  shape(person: Person, record: Stored): Approval {
    return { ...record.approval, can_decide: this.mayDecide(person, record) };
  }

  /** Every request that was made, in the order it was made. */
  all(): readonly Stored[] {
    return this.records;
  }

  /** The request that an approval stands for. */
  recordOf(approvalId: string): Stored | undefined {
    return this.records.find((entry) => entry.approval.id === approvalId);
  }

  /** Puts an approval of kind access into the world as if somebody had asked for it, for a test to decide: `told` says who decides it. */
  adopt(approval: Approval, requester: Person, told: { deciders: string[]; also: string[] }): void {
    const object = approval.object;
    const asked = approval.kind === 'access' ? approval.request.permissions : [];
    this.records.push({ approval, requester, object: { kind: object?.kind ?? 'topic', id: object?.id ?? '', name: object?.name ?? '' }, path: 'nodes', asked, told: { deciders: new Set(told.deciders), also: new Set(told.also) } });
  }

  /** Settles a request as somebody else did, for a test: nothing of the page is changed. */
  settle(approvalId: string, by: UserRef, state: 'approved' | 'denied', decision: unknown, at?:string): Approval | undefined {
    const record = this.recordOf(approvalId);
    if (record === undefined) return undefined;
    record.approval = { ...record.approval, state, decision, decided_at: at??this.access.now(), decided_by: by };
    return record.approval;
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;
    if (route === 'POST /access-requests') return this.create(person, body);
    if ((found = /^GET \/access-requests\/([^/]+)$/.exec(route))) return this.batch(person, found[1] ?? '');
    if (route === 'GET /approvals') return this.list(person, query);
    if ((found = /^GET \/approvals\/([^/]+)$/.exec(route))) {
      const record = this.records.find((entry) => entry.approval.id === found?.[1]);
      return record === undefined ? null : { status: 200, body: this.shape(person, record) };
    }
    if ((found = /^POST \/approvals\/([^/]+)\/decision$/.exec(route))) {
      const record = this.records.find((entry) => entry.approval.id === found?.[1]);
      return record === undefined ? null : this.decide(person, record, body);
    }
    if ((found = /^POST \/approvals\/([^/]+)\/cancel$/.exec(route))) {
      const record = this.records.find((entry) => entry.approval.id === found?.[1]);
      return record === undefined ? null : this.cancel(person, record);
    }
    return null;
  }

  private create(person: Person, body: Record<string, unknown>): Answer {
    const end = typeof body.expires_at === 'string' ? body.expires_at : null;
    if (end !== null && (!Number.isFinite(Date.parse(end)) || Date.parse(end) <= Date.parse(this.access.now()))) throw new Refusal(400, 'request.invalid', 'the requested end must be in the future', { errors: [{ path: 'expires_at', message: 'must be in the future' }] });
    const items = Array.isArray(body.items) ? (body.items as { object?: { kind?: string; id?: string }; permissions?: string[]; options?: unknown; holder?: { kind?: string; id?: string } }[]) : [];
    if (items.length === 0 || items.length > ITEMS_MAX) throw new Refusal(400, 'request.invalid', 'ask for between one and eight things', { errors: [{ path: 'items', message: 'one to eight' }] });
    const waiting = new Set(this.records.filter((record) => record.requester.me.id === person.me.id && record.approval.state === 'pending').map((record) => record.approval.batch_id)).size;
    if (waiting >= this.limit) throw new Refusal(409, 'access.too_many_requests', 'too many requests are waiting', { limit: this.limit, waiting });
    const batch = randomUUID();
    const made: Stored[] = [];
    items.forEach((item, index) => {
      const id = item.object?.id ?? '';
      const kind = item.object?.kind ?? 'topic';
      const path = kind === 'environment' ? 'environments' : 'nodes';
      const row = path === 'nodes' ? this.access.nodeRefOf(id) : null;
      if (typeof row === 'string') throw new Refusal(400, 'request.invalid', 'only places and environments can be asked for here', { errors: [{ path: `items[${index}].object`, message: 'not a place' }] });
      const name = row === null ? this.access.objectName(path, id) : (row.name ?? kind);
      const asked = item.permissions ?? [];
      const recipient = item.holder?.kind === 'user' && item.holder.id !== undefined ? this.access.personOfId(item.holder.id) : person;
      if (recipient === undefined) throw new Refusal(400, 'request.invalid', 'no such selected person', { errors: [{ path: `items[${index}].holder`, message: 'no such person' }] });
      const held = this.access.holdingOn(recipient, id, path).held;
      if (asked.length === 0 || asked.every((name) => held.has(name))) throw new Refusal(400, 'request.invalid', 'nothing to ask', { errors: [{ path: `items[${index}].permissions`, message: 'you hold these already' }] });
      const requester: UserRef = this.access.userRefOf(person);
      const approval = {
        id: randomUUID(),
        kind: 'access',
        state: 'pending',
        title: `${person.me.display_name} asks for ${asked.join(', ')} on ${name}`,
        request: { for_thread: typeof body.for_thread === 'string' ? this.nodeRef(body.for_thread) : null, holder: { kind: 'user', user: this.access.userRefOf(recipient) }, options: null, permissions: asked, reason: typeof body.reason === 'string' ? body.reason : '', ...(end === null ? {} : { expires_at: end }) },
        requester: { user: requester },
        node: row,
        object: { kind, id, name },
        required_permission: asked[0] ?? '',
        thread_id: typeof body.for_thread === 'string' ? body.for_thread : null,
        batch_id: batch,
        item_id: String(index),
        approver_scope: null,
        can_decide: false,
        created_at: this.access.now(),
        decided_at: null,
        decided_automatically: false,
        decided_by: null,
        decision: null,
        expires_at: null,
      } as unknown as Approval;
      made.push({ approval, requester: person, recipient, object: { kind, id, name }, path, asked });
    });
    this.records.push(...made);
    this.changed();
    return { status: 200, body: { batch_id: batch, approvals: made.map((record) => this.shape(person, record)) } };
  }

  private batch(person: Person, batch: string): Answer {
    const mine = this.records.filter((record) => record.approval.batch_id === batch && record.requester.me.id === person.me.id);
    if (mine.length === 0) throw new Refusal(404, 'not_found', 'there is no such request');
    const complete = mine.every((record) => {
      const held = this.access.holdingOn(record.recipient ?? person, record.object.id, record.path).held;
      return record.asked.every((name) => held.has(name));
    });
    return { status: 200, body: { batch_id: batch, approvals: mine.map((record) => this.shape(person, record)), complete } };
  }

  private list(person: Person, query: URLSearchParams): Answer {
    const scope = query.get('scope') ?? 'mine';
    const state = query.get('state') ?? 'pending';
    const kind = query.get('kind');
    let found = this.records;
    if (scope === 'requested') found = found.filter((record) => record.requester.me.id === person.me.id);
    else if (scope === 'others') found = found.filter((record) => this.couldDecide(person, record) && !this.mayDecide(person, record));
    else if (scope.startsWith('node:')) {
      const node = scope.slice('node:'.length);
      this.access.holdingOn(person, node);
      found = found.filter((record) => [record.object.id, ...this.access.ancestorsOf(record.object.id)].includes(node));
    } else found = found.filter((record) => this.mayDecide(person, record));
    found = found.filter((record) => (state === 'pending' ? record.approval.state === 'pending' : record.approval.state !== 'pending') && (kind === null || record.approval.kind === kind));
    return { status: 200, body: { items: found.map((record) => this.shape(person, record)), next_cursor: null } };
  }

  private decide(person: Person, record: Stored, body: Record<string, unknown>): Answer {
    if (record.approval.state !== 'pending') throw new Refusal(409, 'approval.already_decided', 'somebody answered first', { current: this.shape(person, record) });
    if (!this.mayDecide(person, record)) throw this.access.lackingOn(record.object.id, ['access.manage']);
    const approve = body.approve === true;
    const given = Array.isArray(body.permissions) ? (body.permissions as string[]).filter((name) => record.asked.includes(name)) : record.asked;
    // What a test put there has no page of its own to change: the answer is all there is.
    if (record.told !== undefined) {
      record.approval = { ...record.approval, state: approve ? 'approved' : 'denied', decided_at: this.access.now(), decided_by: this.access.userRefOf(person), decision: body };
      this.changed();
      return { status: 200, body: this.shape(person, record) };
    }
    if (approve && given.length > 0) {
      const page = this.access.pageFor(record.object.id, false, record.path);
      const expires = typeof body.expires_at === 'string' ? body.expires_at : null;
      this.access.apply(person, page, {
        rows: given.map((permission) => ({ permission, add: [{ holder: { kind: 'user' as const, id: (record.recipient ?? record.requester).me.id }, ...(expires === null ? {} : { expires_at: expires }) }] })),
      });
    }
    record.approval = { ...record.approval, state: approve ? 'approved' : 'denied', decided_at: this.access.now(), decided_by: this.access.userRefOf(person), decision: { approve, note: body.note ?? null, permissions: approve ? given : [], expires_at: body.expires_at ?? null } };
    this.changed();
    return { status: 200, body: this.shape(person, record) };
  }

  private cancel(person: Person, record: Stored): Answer {
    if (record.requester.me.id !== person.me.id) throw this.access.lackingOn(record.object.id, ['access.manage']);
    record.approval = { ...record.approval, state: 'cancelled' };
    this.changed();
    return { status: 200, body: this.shape(person, record) };
  }

  /** Something was asked or answered: everybody who is connected is told that what waits has changed. */
  private changed(): void {
    this.access.world.notifications.settled();
  }

  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path === '/limit') {
      this.limit = typeof body.waiting === 'number' ? body.waiting : WAITING_MAX;
      return { status: 204 };
    }
    return null;
  }
}
