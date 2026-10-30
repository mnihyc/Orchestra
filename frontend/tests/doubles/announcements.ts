/** Announcements at every place use the fake world's shared access, paths and settings. */
import { randomUUID } from 'node:crypto';
import type { Announcement, AnnouncementDelivered } from '../../src/generated/api-types.ts';
import { userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

/** The production contract's short title and brief body bounds. */
const TITLE_MAX_CHARS = 120;
const BODY_MAX_CHARS = 1500;
const PAGE_DEFAULT = 50;
const PAGE_MAX = 200;

export class AnnouncementsWorld implements Part {
  private readonly world: World;
  private rows: Announcement[] = [];
  constructor(world: World) { this.world = world; }
  reset(): void { this.rows = []; }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const match = /^\/nodes\/([^/]+)\/announcements$/.exec(path);
    if (match === null || (method !== 'GET' && method !== 'POST')) return null;
    const place = match[1] ?? '';
    const places = this.world.places;
    const node = places.rowOf(place);
    if (node === undefined || node.kind === 'thread' || node.deleted) throw new Refusal(404, 'not_found', 'No such place.');
    places.need(person, place, `${node.kind}.${method === 'GET' ? 'view' : 'interact'}`);
    if (method === 'GET') {
      const visible = this.rows.filter((row) => places.pathTo(row.topic_id).some((step) => step.id === place)
        && places.access(person, row.topic_id).permissions.includes('topic.view')
        && (query.get('category') === null || row.category === query.get('category')));
      visible.sort((one, other) => other.created_at.localeCompare(one.created_at) || other.id.localeCompare(one.id));
      const cursor = query.get('cursor');
      const start = cursor === null ? 0 : visible.findIndex((row) => row.id === cursor) + 1;
      if (cursor !== null && start === 0) throw new Refusal(400, 'request.invalid', 'This cursor is not in this list.');
      const limit = Number(query.get('limit') ?? PAGE_DEFAULT);
      if (!Number.isInteger(limit) || limit < 1 || limit > PAGE_MAX) throw new Refusal(400, 'request.invalid', 'Choose a valid page size.');
      const items = visible.slice(start, start + limit);
      return { status: 200, body: { items, next_cursor: start + limit < visible.length ? items.at(-1)?.id ?? null : null } };
    }
    if (node.archived) throw new Refusal(409, 'node.archived', 'This place is archived and read-only.');
    const title = typeof body.title === 'string' ? body.title : '';
    const text = typeof body.body === 'string' ? body.body : '';
    if (title.trim() === '' || Array.from(title).length > TITLE_MAX_CHARS || Array.from(text).length > BODY_MAX_CHARS) throw new Refusal(400, 'request.invalid', 'Give a short title and message.');
    const all = [node, ...places.descendants(place)].filter((row) => row.kind === 'topic' && !row.deleted);
    const topics = all.filter((topic) => !topic.archived && topic.topic?.status !== 'done' && this.world.access.announcementsFollow(topic.id, place));
    const group = randomUUID();
    const made = topics.map((topic): Announcement => {
      const setting = (key: string): unknown => this.world.settings.knows(key) ? this.world.settings.valueAt(topic.id, key).value : null;
      const delivery = body.delivery === 'interrupt' || body.delivery === 'steer' || body.delivery === 'next_turn' ? body.delivery : setting('broadcast.delivery');
      const fallback = body.fallback === 'interrupt' || body.fallback === 'next_turn' ? body.fallback : setting('broadcast.fallback');
      const wake = typeof body.wake_idle === 'boolean' ? body.wake_idle : setting('broadcast.wake_idle') === true;
      const counts: AnnouncementDelivered = { interrupted: 0, steered: 0, queued: 0, deferred: 0 };
      for (const thread of places.threadsOf(topic.id)) {
        const active = ['running', 'starting', 'waiting_approval'].includes(thread.thread?.state ?? 'idle');
        if (body.targets === 'active' && !active) continue;
        if (!active) { if (wake) counts.queued += 1; else counts.deferred += 1; }
        else if (!places.access(person, thread.id).permissions.includes('thread.interact') || delivery === 'next_turn') counts.queued += 1;
        else if (delivery === 'interrupt') counts.interrupted += 1;
        else counts.steered += 1;
      }
      return { id: randomUUID(), topic_id: topic.id, group_id: group, category: body.category === 'milestone' ? 'milestone' : 'notice',
        title, body: text, severity: body.severity === 'important' ? 'important' : 'info', from: { user: userRef(person.me) },
        targets: body.targets === 'active' ? 'active' : 'all', files: [], delivery: delivery === 'interrupt' || delivery === 'next_turn' ? delivery : 'steer',
        fallback: fallback === 'interrupt' ? 'interrupt' : 'next_turn', wake_idle: wake, delivered: counts, created_at: new Date().toISOString(), topic_seq: this.rows.length + 1 };
    });
    this.rows.push(...made);
    return { status: 201, body: { group_id: group, announcements: made, topics_skipped: all.length - topics.length } };
  }
}
