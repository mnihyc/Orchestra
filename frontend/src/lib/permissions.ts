/** What a person holds on a thing, read from what the API says and never from the name of a role, and the words for what they lack.
 *
 * Every object the API returns says which permissions its caller holds on it. A screen enables what
 * those permissions allow. Refusals say what cannot be done and name the place; the advanced
 * permission catalogue retains the exact permission labels and keys.
 */
import type { TFunction } from 'i18next';

import type { Access, NodeKind, UserRef } from '@/generated/api-types';
import { i18n } from '@/i18n';

/** What a screen knows of what its caller holds: the object's `access`, or nothing while it is not loaded. */
export type Held = Pick<Access, 'permissions'> | null | undefined;

/** Whether the permission is among those held. Nothing is held until the object has arrived. */
export function holds(held: Held, permission: string): boolean {
  return held?.permissions.includes(permission) ?? false;
}

/** What the holder of a node's kind may do with it, from the permissions of that kind and the two that cross kinds. */
export function abilities(kind: NodeKind, held: Held) {
  return {
    interact: holds(held, `${kind}.interact`),
    manage: holds(held, `${kind}.manage`),
    // The advanced row of the kind, which whoever manages the node holds with it.
    archive: holds(held, `${kind}.archive`) || holds(held, `${kind}.manage`),
    members: holds(held, 'members.manage'),
    access: holds(held, 'access.manage'),
  };
}

/** The permission a kind of node asks for an act of each level. */
export const needs = {
  interact: (kind: NodeKind): string => `${kind}.interact`,
  manage: (kind: NodeKind): string => `${kind}.manage`,
  archive: (kind: NodeKind): string => `${kind}.archive`,
} as const;

/** The shipped name, when known; a protocol key is never a fallback label. */
export function knownPermissionLabel(name: string): string | null {
  const key = `permissions.${name}`;
  const localized: unknown = i18n.getResource(i18n.resolvedLanguage ?? 'en', 'translation', key);
  if (typeof localized === 'string') return i18n.t(key, { defaultValue: i18n.t('resourceAccessWords.unavailable') });
  const english: unknown = i18n.getResource('en', 'translation', key);
  return typeof english === 'string' ? i18n.getFixedT('en')(key, { defaultValue: i18n.t('resourceAccessWords.unavailable') }) : null;
}

export function permissionLabel(name: string): string {
  return knownPermissionLabel(name) ?? i18n.t('resourceAccessWords.unavailable');
}

/** What cannot be done, in ordinary words; unfamiliar permissions receive no guessed meaning. */
export function permissionDenial(t: TFunction, permission: string, place: string): string {
  switch (permission) {
    case 'org.manage': case 'project.manage': case 'topic.manage': case 'thread.manage':
    case 'doc.manage': case 'environment.manage': case 'provider.manage': case 'host.manage':
    case 'agent.manage': case 'template.manage': case 'mount.manage': case 'mcp.manage': case 'machine.manage':
      return t('access.refusal.manage', { place });
    case 'org.view': case 'project.view': case 'topic.view': case 'thread.view':
    case 'doc.view': case 'environment.view': case 'provider.view': case 'host.view':
    case 'agent.view': case 'template.view': case 'mount.view': case 'mcp.view': case 'machine.view':
      return t('access.refusal.view', { place });
    case 'org.interact': case 'project.interact': case 'topic.interact': case 'thread.interact':
      return t('access.refusal.interact', { place });
    case 'org.archive': case 'project.archive': case 'topic.archive': case 'thread.archive':
      return t('access.refusal.archive', { place });
    case 'doc.edit':
      return t('access.refusal.edit', { place });
    case 'doc.create':
      return t('access.refusal.createDocument', { place });
    case 'provider.use': case 'host.use': case 'agent.use': case 'template.use': case 'mount.use': case 'mcp.use': case 'environment.use':
      return t('access.refusal.use', { place });
    case 'access.manage':
      return t('access.refusal.access', { place });
    case 'access.own':
      return t('access.refusal.own', { place });
    case 'settings.manage':
      return t('access.refusal.settings', { place });
    case 'members.manage':
      return t('access.refusal.members', { place });
    case 'ports.manage':
      return t('access.refusal.ports', { place });
    case 'audit.view':
      return t('access.refusal.audit', { place });
    case 'files.view':
      return t('access.refusal.readFiles', { place });
    case 'files.manage':
      return t('access.refusal.changeFiles', { place });
    case 'terminal.view':
      return t('access.refusal.watchTerminals', { place });
    case 'terminal.manage':
      return t('access.refusal.controlTerminals', { place });
    default:
      return t('access.takes', { place });
  }
}

/** Why an act is unavailable on this place, with its owner as the way to ask when named. */
export function denial(t: TFunction, permission: string, place: string, owner: Pick<UserRef, 'display_name'> | null | undefined): string {
  const reason = permissionDenial(t, permission, place);
  return owner === null || owner === undefined ? reason : t('access.askOwner', { reason, name: owner.display_name });
}
