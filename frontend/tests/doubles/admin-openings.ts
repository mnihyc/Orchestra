/** Native logical document openings, using the world's current read authority and trace producer. */
import { randomUUID } from 'node:crypto';
import { type Answer, type Person, Refusal, type World } from './world.ts';

export interface DocumentOpening {
  person: Person;
  session: string;
  nonce: string;
  document: { id: string; name: string };
  requestId: string | null;
}

const KINDS = new Set(['org', 'project', 'topic', 'thread', 'document', 'document_container', 'provider', 'agent', 'environment', 'host', 'machine', 'template', 'mcp_server', 'mount', 'chat', 'user']);
const ROUTES: Readonly<Record<string, string>> = { orgs: 'org', projects: 'project', topics: 'topic', threads: 'thread', documents: 'document', chats: 'chat', users: 'user', environments: 'environment', providers: 'provider', agents: 'agent', hosts: 'host', machines: 'machine', templates: 'template', mounts: 'mount', 'mcp-servers': 'mcp_server' };
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

export class AdminOpenings {
  private readonly world: World;
  constructor(world: World) { this.world = world; }

  nonce(value: unknown, field: string): string {
    // The generated wire uses UUID text, including nil; never infer a version,
    // ordinal, route, display name or another browser's session from it.
    if (typeof value !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)) {
      throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path: field, message: 'must be a UUID' }] });
    }
    return value.toLowerCase();
  }

  private kind(value: unknown): void {
    if (typeof value !== 'string' || !KINDS.has(value)) throw new Refusal(400, 'request.invalid', 'is not a kind of readable object', { errors: [{ path: 'kind', message: 'is not a kind of readable object' }] });
    if (value !== 'document') throw new Refusal(400, 'request.value_refused', 'the double has no private-opening authorization supplier for this kind');
  }

  private access(context: ReturnType<World['openingIdentity']>, id: string) {
    const access = this.world.documents.openingAccess(context.person, id);
    const doc = this.world.documents.docs.get(id);
    if (access === null && !context.person.me.is_system_admin && doc !== undefined
      && this.world.places.roleAt(context.person, doc.nodeId) === 'admin') {
      throw new Refusal(400, 'request.value_refused', 'the double has no opening authorization supplier for private node-administrator reach');
    }
    return access;
  }

  cached(authorization: string | undefined, body: Record<string, unknown>, requestId: string | null = null): Answer {
    if (Object.keys(body).length !== 3 || Object.keys(body).some((key) => !['kind', 'id', 'opening_id'].includes(key))) throw new Refusal(400, 'request.invalid', 'the opening body must contain only kind, id and opening_id');
    this.kind(body.kind);
    const id = this.nonce(body.id, 'id'), nonce = this.nonce(body.opening_id, 'opening_id');
    const context = this.world.openingIdentity(authorization);
    // As in private_place, Off reveals no existence and makes no audit claim.
    if (context.asAdmin) {
      const document = this.access(context, id);
      if (document?.place === 'system') this.world.trace.openedDocument({ ...context, nonce, document, requestId });
    }
    return { status: 204 };
  }

  finishJson(authorization: string | undefined, method: string, path: string, query: URLSearchParams, value: unknown, header: unknown, requestId: string | null): void {
    if (method !== 'GET' || query.get('inspect_deleted') === 'true' || !record(value)) return;
    const parts = path.split('/').filter(Boolean), kind = parts.length === 2 ? ROUTES[parts[0] ?? ''] : undefined;
    if (kind === undefined) return;
    const access = record(value.access) ? value.access : record(value.can) ? value.can : null;
    const reach = access !== null && record(access.admin_access) ? access.admin_access : null;
    const place = reach?.place ?? (value.inspected === null ? undefined : value.inspected);
    if (place === undefined) return;
    this.kind(kind);
    if (place !== 'system') throw new Refusal(400, 'request.value_refused', 'the double has no opening authorization supplier for this private place');
    const id = this.nonce(parts[1], 'id');
    if (typeof value.id !== 'string' || value.id.toLowerCase() !== id) throw new Refusal(500, 'internal', 'the private object reply has no matching identity');
    const nonce = header === undefined ? randomUUID() : this.nonce(header, 'x-orch-opening');
    const context = this.world.openingIdentity(authorization);
    if (!context.asAdmin) throw new Refusal(403, 'authz.denied', 'the private opening no longer has administrative reach');
    const document = this.access(context, id);
    if (document === null) throw new Refusal(403, 'authz.denied', 'the private opening is no longer readable');
    if (document.place === 'system') this.world.trace.openedDocument({ ...context, nonce, document, requestId });
  }

  /** Called only for an actual acknowledged doc subscription, never a request attempt. */
  live(authorization: string | undefined, id: string, nonce: string): void {
    const actor = this.world.browserPerson(authorization);
    const document = this.world.documents.openingAccess(actor, id);
    if (document?.place !== 'system') return;
    const context = this.world.openingIdentity(authorization);
    if (!context.asAdmin) throw new Refusal(403, 'authz.denied', 'the private opening no longer has administrative reach');
    this.world.trace.openedDocument({ ...context, nonce, document, requestId: null });
  }
}
