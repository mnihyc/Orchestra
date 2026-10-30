/** The Access tab of a resource: what the person holds on it and through what, and the way to its permission page.
 */
import type { TFunction } from 'i18next';
import { useQuery } from '@tanstack/react-query';
import { ShieldCheck } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { permissionsQuery, type AccessTarget } from '@/api/access';
import { permissionNameOf } from '@/features/access/names';
import { openAccess } from '@/features/access/AccessDialog';
import type { Holder, PermissionDef, ResourceAccess as Held } from '@/generated/api-types';
import { knownPermissionLabel } from '@/lib/permissions';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { actionName } from '@/ui/actions';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';

/** The permissions a resource's page is made of, which the catalogue of this part has words for. */
const WORDED = [
  'provider.view',
  'provider.use',
  'provider.manage',
  'provider.bypass',
  'agent.view',
  'agent.use',
  'agent.manage',
  'environment.view',
  'environment.use',
  'environment.manage',
  'template.view',
  'template.use',
  'template.manage',
  'host.view',
  'host.use',
  'host.manage',
  'machine.view',
  'machine.use',
  'machine.manage',
  'mount.view',
  'mount.use',
  'mount.manage',
  'mcp.view',
  'mcp.use',
  'mcp.manage',
  'skill.view',
  'skill.use',
  'skill.manage',
  'access.manage',
  'access.own',
  'access.external',
] as const;

type Worded = (typeof WORDED)[number];

const isWorded = (name: string): name is Worded => (WORDED as readonly string[]).includes(name);

/** A permission uses catalogue words or its server-supplied name; a missing name remains unknown. */
export function resourcePermission(t: TFunction, name: string, definition?: Pick<PermissionDef, 'name' | 'label'>): string | null {
  if (isWorded(name)) return t(`resources.permissions.${name}`);
  // Missing-name display text must not stand in for a resolved name and suppress catalogue discovery.
  const shared = definition === undefined ? knownPermissionLabel(name) : permissionNameOf(definition);
  return shared === null || shared === name || shared.trim() === '' ? null : shared;
}

/** What gives a person their level, as a sentence. */
function through(t: TFunction, entry: Held['through'][number]): string {
  if (entry.kind === 'source_owner') return t('access.sourceOwner');
  if (entry.kind === 'owner') return t('resources.access.through.owner');
  const holder: Holder = entry.holder;
  switch (holder.kind) {
    case 'role':
      return t('resources.access.through.role', { role: t(`roles.${holder.role}`), place: holder.place ?? holder.at?.name ?? t('resources.access.through.itsOrg') });
    case 'group':
      return t('resources.access.through.group', { name: `${holder.group.name}, ${holder.group.place ?? t('access.place.hidden')}` });
    case 'members':
      return t('access.holder.membersOf', { place: holder.node.name ?? t('access.place.hidden') });
    case 'user':
      return t('resources.access.through.user');
    case 'node':
      return t('resources.access.through.node', { place: holder.node.name ?? '' });
    case 'agents':
      return t('resources.access.through.agents', { place: holder.node.name ?? '' });
    case 'everyone':
      return t('resources.access.through.everyone');
  }
}

export function ResourceAccess({ access, target }: { access: Held; target: AccessTarget }) {
  const { t } = useTranslation();
  const needsNames = access.permissions.some((permission) => resourcePermission(t, permission) === null);
  const catalogue = useQuery({ ...permissionsQuery, enabled: needsNames });
  const definitions = new Map(catalogue.data?.permissions.map((def) => [def.name, def]));
  return (
    <div className="flex flex-col gap-6">
      <Section id="access-hold" title={t('resources.access.hold')} lead={t('resources.access.holdLead')}>
        {access.permissions.length === 0 ? (
          <p className="text-sm text-text-muted">{t('resources.access.holdNothing')}</p>
        ) : (
          <ul aria-label={t('resources.access.hold')} className="flex flex-wrap gap-2">
            {access.permissions.map((permission) => (
              <li key={permission}>
                <Badge className="h-6 px-2 text-sm">{resourcePermission(t, permission, definitions.get(permission)) ?? t(catalogue.isPending ? 'resourceAccessWords.reading' : 'resourceAccessWords.unavailable')}</Badge>
              </li>
            ))}
          </ul>
        )}
        {needsNames && catalogue.isError && <ErrorState compact error={catalogue.error} onRetry={() => void catalogue.refetch()} />}
        {access.administers && <p className="text-sm text-text-muted">{t('resources.access.administers')}</p>}
      </Section>
      <Section id="access-through" title={t('resources.access.throughTitle')}>
        {access.through.length === 0 ? (
          <p className="text-sm text-text-muted">{t('resources.access.throughNothing')}</p>
        ) : (
          <ul className="flex flex-col gap-1 text-base text-text">
            {access.through.map((entry, index) => (
              // What gives a level is listed as the server gave it and never reordered.
              <li key={index}>{through(t, entry)}</li>
            ))}
          </ul>
        )}
      </Section>
      <EmptyState
        compact
        icon={<ShieldCheck aria-hidden />}
        title={t('resources.access.page')}
        description={t('resources.access.pageNote')}
        action={
          <Button action="permissions" variant="secondary" onClick={() => openAccess(target, { share: false })}>
            {actionName('permissions')}
          </Button>
        }
      />
    </div>
  );
}
