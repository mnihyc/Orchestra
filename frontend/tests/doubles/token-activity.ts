/** Native-shaped deterministic activity fixtures. Seeded records are fixture data, never proof of key execution. */
import type { ApiTokenActivity, ApiTokenActivityPage } from '../../src/generated/api-types';
import { Refusal, type Person, type World } from './world.ts';

export interface RecordedKeyCall {
  row: ApiTokenActivity;
  /** Recorded governing orgs for EACH facet; the org reader never receives another org's facets. */
  organizations: readonly (readonly string[])[];
}
interface Stored extends RecordedKeyCall { sequence: number }
const resources = { provider: 'providers', agent: 'agents', environment: 'environments', host: 'hosts', template: 'templates' } as const;
const badCursor = (): never => { throw new Refusal(400, 'request.invalid', 'is not a cursor of this list', { errors: [{ path: 'cursor', message: 'is not a cursor of this list' }] }); };

export class TokenActivity {
  private owners = new Map<string, string>();
  private rows = new Map<string, Stored[]>();
  private sequence = 0;
  private readonly world: World;
  constructor(world: World) { this.world = world; }
  remember(id: string, account: string): void { this.owners.set(id, account); }
  /** Setup-only input, typed with the actual public DTO. Revocation deliberately does not erase ownership/history. */
  seed(id: string, calls: readonly RecordedKeyCall[]): void {
    this.rows.set(id, []);
    for (const call of calls) this.record(id, call);
  }
  record(id: string, call: RecordedKeyCall): void { this.rows.set(id, [...(this.rows.get(id) ?? []), { ...structuredClone(call), sequence: ++this.sequence }]); }
  private name(person: Person, row: ApiTokenActivity['facets'][number]): string | null {
    const id = row.target.id;
    if (id === undefined || id === null) return null;
    const ordinary: Person = { ...person, me: { ...person.me, is_system_admin: false } };
    const kind = row.target.kind;
    if (kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread') {
      const target = this.world.places.rowOf(id);
      return target !== undefined && this.world.places.access(ordinary, id).permissions.length > 0 ? target.name : null;
    }
    if (kind === 'document') {
      const target = this.world.documents.docs.get(id);
      return target !== undefined && !target.deleted && this.world.access.holdingOn(ordinary, id, 'documents').held.size > 0 ? target.title : null;
    }
    if (kind === 'provider' || kind === 'agent' || kind === 'environment' || kind === 'host' || kind === 'template') {
      const target = this.world.resources.describe(kind, id);
      return target !== null && this.world.access.holdingOn(ordinary, id, resources[kind]).held.size > 0 ? target.name : null;
    }
    // This double has no native name producer for the remaining kinds; never trust a seeded historical name.
    return null;
  }
  page(person: Person, id: string, org: string | undefined, query: URLSearchParams): ApiTokenActivityPage {
    if (org === undefined) {
      if (this.owners.get(id) !== person.me.id) throw new Refusal(404, 'not_found', 'no such key');
    } else this.world.places.need(person, org, 'members.manage');
    const rawLimit = query.get('limit'), parsed = rawLimit === null ? 50 : Number(rawLimit);
    if ([...query.keys()].some((key) => key !== 'limit' && key !== 'cursor') || (rawLimit !== null && !/^\d+$/.test(rawLimit)) || !Number.isInteger(parsed) || parsed < 0 || parsed > 4_294_967_295) throw new Refusal(400, 'request.invalid', 'limit is not an unsigned integer');
    const limit = Math.max(1, Math.min(100, parsed)), rawCursor = query.get('cursor');
    let snapshot = this.sequence, after: { at: number; id: string } | undefined;
    if (rawCursor !== null) {
      const [version, key, place, watermark, at, rowId, extra] = rawCursor.split('|');
      if (version !== '1' || key !== id || place !== (org ?? '-') || extra !== undefined || rowId === undefined || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(rowId) || watermark === undefined || !/^\d+$/.test(watermark) || at === undefined || !/^-?\d+$/.test(at) || !Number.isSafeInteger(Number(watermark)) || Number(watermark) < 0 || !Number.isSafeInteger(Number(at))) return badCursor();
      snapshot = Number(watermark); after = { at: Number(at), id: rowId };
    }
    const cutoff = Date.now() - 7 * 86_400_000;
    const rows = (this.rows.get(id) ?? []).filter((entry) => Date.parse(entry.row.at) > cutoff)
      .sort((one, other) => Date.parse(other.row.at) - Date.parse(one.row.at) || other.row.id.localeCompare(one.row.id)).slice(0, 100)
      .filter((entry) => entry.sequence <= snapshot && (org === undefined || entry.organizations.some((orgs) => orgs.includes(org))))
      .filter((entry) => after === undefined || Date.parse(entry.row.at) * 1000 < after.at || (Date.parse(entry.row.at) * 1000 === after.at && entry.row.id < after.id));
    const selected = rows.slice(0, limit), last = selected.at(-1);
    const showIp = org === undefined || this.world.places.access(person, org).permissions.includes('audit.view');
    return { items: selected.map(({ row, organizations }) => ({ ...structuredClone(row), ip: showIp ? row.ip : null,
      facets: row.facets.filter((_facet, index) => org === undefined || organizations[index]?.includes(org)).map((facet) => ({ ...structuredClone(facet), target_name: this.name(person, facet) })),
    })), next_cursor: rows.length > limit && last !== undefined ? `1|${id}|${org ?? '-'}|${snapshot}|${Date.parse(last.row.at) * 1000}|${last.row.id}` : null };
  }
}
