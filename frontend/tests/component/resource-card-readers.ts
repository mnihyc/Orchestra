/** Native auxiliary readers used by an opened card; each path has its own contract-shaped fixture. */
import { pathOf } from '@/api/access';
import { org } from '../doubles/fixtures';
import { ADA, CATALOGUE, pageFor } from '../unit/general-access-fixtures';

export interface CardReadOptions { params?: { path?: { id?: string; object?: string } } }
export function resourceCardRead(path: string, options?: CardReadOptions) {
  const reply = <T,>(data: T) => ({ data, error: undefined, response: new Response(null, { status: 200 }) });
  if (path === '/permissions') return reply(CATALOGUE.reply);
  if (path === '/nodes/{id}/defaults') {
    const id = options?.params?.path?.id;
    if (id === undefined) throw new Error('A resource defaults read needs its actual home id.');
    const page = pageFor('org');
    return reply({ ...page, object: { ...page.object, id }, page: 'defaults' as const, rows: [] });
  }
  if (path === '/nodes/{id}') {
    const id = options?.params?.path?.id;
    if (id === undefined) throw new Error('A card breadcrumb read needs its actual node id.');
    return reply(org(id, 'acme', 'Acme', ADA));
  }
  if (path === '/{object}/{id}/permissions') {
    const kind = (['org', 'project', 'topic', 'thread', 'provider', 'host', 'environment', 'mount', 'mcp_server', 'template', 'agent', 'skill'] as const)
      .find((kind) => pathOf(kind) === options?.params?.path?.object);
    if (kind === undefined) throw new Error('A card sharing read needs its actual resource kind.');
    const page = pageFor(kind);
    return reply({ ...page, object: { ...page.object, id: options?.params?.path?.id ?? page.object.id } });
  }
  return null;
}
