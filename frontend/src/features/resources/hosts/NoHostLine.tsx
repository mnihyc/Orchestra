/** The one line said where a person may use no host for what they are making, naming what fixes it. Someone who
 * administers turns on As admin, or names themself in Use on a host's Access; anyone else asks the owner of a host they
 * can see. A provider's container runs as its owner, so for it a host counts only where the owner may use it as
 * themself: Use that only As admin gives does not carry over to the container.
 */
import { Link } from '@tanstack/react-router';
import { Trans, useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { AdminModeEnableAction, useAdminMode } from '@/features/access/AdminMode';
import { usesAsThemself } from '@/features/resources/choiceAvailability';
import type { Host, PlaceHost } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { UserChip } from '@/ui/UserChip';

export type NoHostKind = 'environment' | 'provider';

const WORDS = {
  environment: { admin: 'hosts.noneFor.environment.admin', adminOn: 'hosts.noneFor.environment.adminOn', owner: 'hosts.noneFor.environment.owner', ask: 'hosts.noneFor.environment.ask' },
  provider: { admin: 'hosts.noneFor.provider.admin', adminOn: 'hosts.noneFor.provider.adminOn', owner: 'hosts.noneFor.provider.owner', ask: 'hosts.noneFor.provider.ask' },
} as const;

export interface NoHostLineProps {
  kind: NoHostKind;
  /** The hosts the person can see, with their owners. */
  hosts: readonly (Host | PlaceHost)[];
  /** Muted, and no announcement, where something else already serves or already says what is missing; in the warning tone, as a
   * status, where it blocks the default. */
  quiet?: boolean;
}

export function NoHostLine({ kind, hosts, quiet = false }: NoHostLineProps) {
  const { t } = useTranslation();
  const mode = useAdminMode();
  const me = useStore(client.session, (state) => state.userId);
  const words = WORDS[kind];
  // A host the person sees and may not use for it: whose Access an administrator opens, or, of somebody else, whose owner to ask.
  const may = (entry: Host) => kind === 'provider' ? usesAsThemself(entry) : holds(entry.access, 'host.use');
  const closed = hosts.filter((entry): entry is Host => !('automatic' in entry) && !may(entry));
  const className = `flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm ${quiet ? 'text-text-muted' : 'text-warning-text'}`;
  if (mode.data?.available === true) {
    const off = !mode.data.as_admin;
    const open = closed[0];
    return <span role={quiet ? undefined : 'status'} className={className} data-default-host="none" data-no-host="admin">
      <span>{t(off ? words.admin : words.adminOn)}</span>
      {off ? <AdminModeEnableAction /> : open !== undefined && (
        <Link to="/resources/hosts/$id" params={{ id: open.id }} search={{ tab: 'access' }} className="text-accent-text hover:underline">
          {t('hosts.noneFor.access', { host: open.name })}
        </Link>
      )}
    </span>;
  }
  const theirs = closed.find((entry) => entry.owner !== null && entry.owner.id !== me);
  return <span role={quiet ? undefined : 'status'} className={className} data-default-host="none" data-no-host="member">
    {theirs !== undefined && theirs.owner !== null
      ? <span className="inline-flex flex-wrap items-center gap-1"><Trans i18nKey={words.owner} values={{ host: theirs.name }} components={{ person: <UserChip user={theirs.owner} size={24} /> }} /></span>
      : t(words.ask)}
  </span>;
}
