/** Personal pages and work expose only what this fake world's current reader may already view. */
import type { MeProfileUpdateBody, NodeRef, ProfileActivity, ProfilePin, ProfilePinInput, ProfileSettings, UserProfile, UserShare } from '../../src/generated/api-types.ts';
import { userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

interface Stored { bio: string; pronouns: string; links: UserProfile['links']; settings: ProfileSettings; pins: ProfilePinInput[] }
interface Work { username: string; thread: string; topic: string; day: string; turns: number; last_at: string }
const DAY_MS = 24 * 60 * 60 * 1000;
const CALENDAR_DAYS = 53 * 7;
const MAX_ITEMS = 6;
const ref = (node: { id: string; kind: NodeRef['kind']; name: string }): NodeRef => ({ id: node.id, kind: node.kind, name: node.name, redacted: false });

export class Profiles implements Part {
  private pages = new Map<string, Stored>();
  private work: Work[] = [];
  private inspections = new Set<string>();
  private readonly world: World;
  constructor(world: World) { this.world = world; }
  reset(): void { this.pages.clear(); this.work = []; this.inspections.clear(); }
  private stored(username: string): Stored {
    let page = this.pages.get(username);
    if (page === undefined) { page = { bio: '', pronouns: '', links: [], settings: { show_time: true, show_activity: true, audience: 'system' }, pins: [] }; this.pages.set(username, page); }
    return page;
  }
  private target(username: string): Person {
    const target = this.world.people.get(username);
    if (target?.me.kind !== 'human') throw new Refusal(404, 'not_found', 'there is no page at this address');
    return target;
  }
  private roots(target: Person) { return this.world.orgsOf(target).filter((org) => org.id !== target.me.personal_org_id); }
  private opening(reader: Person, target: Person): NodeRef | null {
    const page = this.stored(target.me.username);
    const shared = this.roots(target).some((org) => this.world.places.isMember(reader, org.id));
    if (reader.me.id === target.me.id || page.settings.audience === 'system' || (page.settings.audience === 'orgs' && shared)) return null;
    const org = this.roots(target).find((org) => this.world.places.access(reader, org.id).permissions.includes('members.manage'));
    if (org === undefined) throw new Refusal(404, 'not_found', 'there is no page at this address');
    this.inspections.add(`${reader.me.id}:${target.me.id}:${new Date().toISOString().slice(0, 10)}`);
    return ref(org);
  }
  private pin(reader: Person, input: ProfilePinInput): ProfilePin | null {
    if ('node_id' in input) {
      try { return { node: this.world.places.node(reader, input.node_id), document: null }; }
      catch (error) { if (error instanceof Refusal) return null; throw error; }
    }
    const doc = this.world.documents.readable(reader).find((doc) => doc.id === input.document_id);
    return doc === undefined ? null : { node: null, document: this.world.documents.ref(reader, doc) };
  }
  private profile(reader: Person, target: Person): UserProfile {
    const inspected = this.opening(reader, target);
    const page = this.stored(target.me.username);
    const roots = this.roots(target);
    const member_of = roots.filter((org) => this.world.places.isMember(reader, org.id)).flatMap((org) => {
      try { return [ref(this.world.places.node(reader, org.id))]; } catch (error) { if (error instanceof Refusal) return []; throw error; }
    });
    let personal_org: NodeRef | null = null;
    if (target.me.personal_org_id !== null) {
      try { personal_org = ref(this.world.places.node(reader, target.me.personal_org_id)); } catch (error) { if (!(error instanceof Refusal)) throw error; }
    }
    const is_me = reader.me.id === target.me.id;
    return { user: userRef(target.me), joined_at: target.me.created_at, is_me, bio: page.bio, pronouns: page.pronouns, links: page.links,
      local_time: page.settings.show_time ? { timezone: target.me.timezone, utc_offset_minutes: 0 } : null,
      member_of, personal_org, pins: page.pins.flatMap((input) => { const pin = this.pin(reader, input); return pin === null ? [] : [pin]; }),
      activity: is_me || page.settings.show_activity, can_message: !is_me && member_of.length > 0 && !target.suspended,
      inspected, settings: is_me ? { ...page.settings } : null };
  }
  private activity(reader: Person, target: Person): ProfileActivity {
    this.opening(reader, target);
    if (reader.me.id !== target.me.id && !this.stored(target.me.username).settings.show_activity) throw new Refusal(404, 'not_found', 'there is no page at this address');
    const to = new Date().toISOString().slice(0, 10);
    const from = new Date(Date.parse(`${to}T00:00:00Z`) - (CALENDAR_DAYS - 1) * DAY_MS).toISOString().slice(0, 10);
    const days = new Map<string, number>(); const recent = new Map<string, string>();
    for (const work of this.work) {
      if (work.username !== target.me.username || work.day < from || work.day > to) continue;
      try { this.world.places.node(reader, work.thread); } catch (error) { if (error instanceof Refusal) continue; throw error; }
      days.set(work.day, (days.get(work.day) ?? 0) + work.turns);
      recent.set(work.topic, [recent.get(work.topic) ?? '', work.last_at].sort().at(-1) ?? work.last_at);
    }
    const topics = [...recent].flatMap(([id, last_at]) => { try { return [{ node: ref(this.world.places.node(reader, id)), last_at }]; } catch (error) { if (error instanceof Refusal) return []; throw error; } }).sort((a, b) => b.last_at.localeCompare(a.last_at)).slice(0, 8);
    return { from, to, days: [...days].sort(([a], [b]) => a.localeCompare(b)).map(([day, turns]) => ({ day, turns })), total: [...days.values()].reduce((sum, n) => sum + n, 0), recent: topics };
  }
  handle(reader: Person, method: string, path: string, _query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const shares = /^\/users\/([^/]+)\/shares$/.exec(path);
    if (method === 'GET' && shares !== null) {
      const target = [...this.world.people.values()].find((person) => person.me.id === shares[1] && person.me.kind === 'human');
      if (target === undefined) throw new Refusal(404,'not_found','no such person');
      if (target.me.id === reader.me.id) return {status:200,body:{items:[],next_cursor:null}};
      const items = this.world.access.directShares(reader,target).flatMap((id): UserShare[] => {
        try { this.world.places.node(target,id); return [{kind:'node',node:ref(this.world.places.node(reader,id))}]; }
        catch (error) { if (error instanceof Refusal) return []; throw error; }
      });
      const cursor = _query.get('cursor');
      if (cursor !== null && !/^node\|[\da-f-]+$/i.test(cursor)) throw new Refusal(400,'request.invalid','not a cursor of this list');
      const after = cursor?.slice('node|'.length);
      const visible = items.filter((item) => item.kind === 'node' && (after === undefined || item.node.id > after));
      const limit = Math.max(1,Math.min(200,Number(_query.get('limit') ?? 50)));
      const page = visible.slice(0,limit);
      const last = page.at(-1);
      return {status:200,body:{items:page,next_cursor:visible.length > limit && last?.kind === 'node' ? `node|${last.node.id}` : null}};
    }
    const match = /^\/profiles\/([^/]+)(\/activity)?$/.exec(path);
    if (method === 'GET' && match?.[1] !== undefined) { const target = this.target(decodeURIComponent(match[1])); return { status: 200, body: match[2] === undefined ? this.profile(reader, target) : this.activity(reader, target) }; }
    if ((method !== 'PATCH' || path !== '/me/profile') && (method !== 'PUT' || path !== '/me/profile/pins')) return null;
    const target = this.target(reader.me.username); const page = this.stored(target.me.username);
    if (path.endsWith('/pins')) {
      const inputs = body.pins as ProfilePinInput[];
      if (!Array.isArray(inputs) || inputs.length > MAX_ITEMS || inputs.some((pin) => this.pin(reader, pin) === null)) throw new Refusal(400, 'request.invalid', 'each pin must be something you may view');
      page.pins = structuredClone(inputs);
    } else {
      const patch = body as MeProfileUpdateBody;
      // Rust's optional PATCH fields treat JSON null like an omitted field.
      if (patch.bio !== undefined && patch.bio !== null) page.bio = patch.bio;
      if (patch.pronouns !== undefined && patch.pronouns !== null) page.pronouns = patch.pronouns;
      if (patch.links !== undefined && patch.links !== null) page.links = patch.links;
      if (patch.show_time !== undefined && patch.show_time !== null) page.settings.show_time = patch.show_time;
      if (patch.show_activity !== undefined && patch.show_activity !== null) page.settings.show_activity = patch.show_activity;
      if (patch.audience !== undefined && patch.audience !== null) page.settings.audience = patch.audience;
    }
    return { status: 200, body: this.profile(reader, target) };
  }
  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path === '/page') {
      const username = String(body.username); const page = this.stored(username);
      if (body.profile !== undefined) Object.assign(page, body.profile);
      return { status: 204 };
    }
    if (path === '/work') { this.work = structuredClone(body.work as Work[]); return { status: 204 }; }
    if (path === '/inspections') return { status: 200, body: [...this.inspections] };
    return null;
  }
}
