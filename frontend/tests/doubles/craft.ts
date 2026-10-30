/** Additional readable document permission pages for journeys through the shared access controls. */
import type { Document, PermissionPage } from '../../src/generated/api-types.ts';
import type { Answer, Person, World } from './world.ts';

export class Craft {
  private readonly world: World;
  constructor(world: World) { this.world = world; }

  /** Adds only the document GET absent from the original double; mutations keep their original handler. */
  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const match = /^\/documents\/([^/]+)\/permissions$/.exec(path);
    if (method !== 'GET' || match === null) return null;
    const id = match[1] ?? '';
    const answer = this.world.documents.handle(person, 'GET', `/documents/${id}`, query, body);
    if (answer === null) return null;
    if (answer.status !== 200) return answer;
    const doc = answer.body as Document;
    const parent = this.world.access.handle(person, 'GET', `/nodes/${doc.node.id}/permissions`, query, body)?.body as PermissionPage | undefined;
    if (parent === undefined) return null;
    const manages = doc.access.permissions.includes('access.manage');
    const page: PermissionPage = {
      ...parent,
      object: { kind: 'document', id: doc.id, name: doc.title },
      state: doc.page,
      covers: null,
      depends: [],
      reaches: null,
      followers: null,
      owner: doc.owner,
      owners_writable: manages,
      admins: { ...parent.admins, of: [doc.node, ...parent.admins.of] },
      owners: parent.owners.map((entry) => ({ ...entry, holder: { kind: 'user', user: doc.owner } })),
      gives: { ...parent.gives, roles: [], choices: [ ...(doc.access.permissions.includes('doc.view') ? ['full_view' as const] : []), ...(doc.access.permissions.includes('doc.edit') ? ['full_interact' as const] : []), ...(doc.access.permissions.includes('doc.manage') ? ['full_manage' as const] : []) ], agents: [ ...(doc.access.permissions.includes('doc.view') ? ['view' as const] : []), ...(doc.access.permissions.includes('doc.edit') ? ['edit' as const] : []), ...(doc.access.permissions.includes('doc.manage') ? ['manage' as const] : []) ] },
      rows: parent.rows.filter((row) => (row.permission.startsWith('doc.') && row.permission !== 'doc.create') || row.permission.startsWith('access.')).map((row) => ({ ...row, writable: manages, can_give: doc.access.permissions.includes(row.permission), inherit: doc.page !== 'restricted', inherited: doc.page === 'restricted' ? null : row.inherited })),
    };
    return { status: 200, body: page };
  }
}
