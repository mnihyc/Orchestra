/** The catalogue of permissions as the real API serves it at `GET /permissions`: sixty permissions with their levels and holders, the three everyday choices and the four roles.
 *
 * It was read from a running system and is kept as it was, apart from the JSON Schemas of the options
 * an entry may carry, which only the permission of a provider's Use keeps (the pages draw no others).
 * The fake API answers with it, and the pages are drawn from it and from nothing else; the unit tests
 * read the same data from `catalogue.json`.
 */
import type { PermissionCatalogueGetReply } from '../../src/generated/api-types.ts';
import data from './catalogue.json' with { type: 'json' };
import { PORTS_PERMISSIONS } from './ports.ts';

export const CATALOGUE = data as unknown as PermissionCatalogueGetReply;
CATALOGUE.permissions.push(...PORTS_PERMISSIONS);
for (const choice of CATALOGUE.choices) {
  const rows = choice.permissions.environment;
  if (rows !== undefined) rows.push('ports.view', ...(choice.key === 'full_manage' ? ['ports.manage'] : []));
}
