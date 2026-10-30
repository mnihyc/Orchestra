/** What a permission is called in the reader's language: shared words, the words of this page, or a readable label the server gives it.
 *
 * The server's catalogue of permissions carries a label for each permission in English, which is
 * what an administrator will look for; the catalogue of texts translates the ones it has words for,
 * so that a permission is named the same way in a refusal, on a button that is off and on this page.
 * A permission with no readable label uses an unavailable-name sentence, never a protocol key.
 */
import { useQuery } from '@tanstack/react-query';

import { permissionsQuery } from '@/api/access';
import type { PermissionDef } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { knownPermissionLabel, permissionLabel } from '@/lib/permissions';

/** The name of a permission whose definition is at hand. */
export function permissionNameOf(def: Pick<PermissionDef, 'name' | 'label'>, folder = false): string {
  if (folder && def.name === 'project.archive') return i18n.t('craftAccess.archiveFolder');
  if (def.name === 'ports.view') return i18n.t('ports.viewPermission');
  if (def.name === 'ports.manage') return i18n.t('ports.managePermission');
  const shared = knownPermissionLabel(def.name);
  if (shared !== null) return shared;
  const key = `access.permission.${def.name}`;
  // The key is built from a name the server gave, so it is not one the catalogue's types know.
  return i18n.exists(key) ? i18n.t(key, { defaultValue: i18n.t('resourceAccessWords.unavailable') }) : def.label.trim() === '' || def.label === def.name ? i18n.t('resourceAccessWords.unavailable') : def.label;
}

/** The name of a permission by its key, from the catalogue the page was drawn from. */
export function usePermissionName(enabled=true): (permission: string) => string {
  const catalogue = useQuery({...permissionsQuery,enabled}).data;
  return (permission) => {
    const def = catalogue?.permissions.find((entry) => entry.name === permission);
    return def === undefined ? permissionLabel(permission) : permissionNameOf(def);
  };
}
