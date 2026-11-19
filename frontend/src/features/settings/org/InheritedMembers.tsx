/** A department's domain members are not everyone who can work there: show the authoritative role holders above it too. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { rolesQuery } from '@/api/access';
import { placeName } from '@/features/access/holders';
import { ROLE_ORDER } from '@/features/access/model';
import { SubjectView } from '@/features/access/Roles';
import { matches } from '@/features/finder/ranking';
import type { Member, Org, RoleHeld } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

/** Same holder, one row: the highest declared role and its actual source, never an inferred effective permission. */
function highest(rows: readonly RoleHeld[]): RoleHeld[] {
  const found = new Map<string, RoleHeld>();
  for (const row of rows) {
    const key = `${row.subject.kind}:${row.subject.id}:${row.subject.role ?? ''}`;
    const previous = found.get(key);
    if (previous === undefined || ROLE_ORDER.indexOf(row.role) > ROLE_ORDER.indexOf(previous.role)) found.set(key, row);
  }
  return [...found.values()];
}

function InheritedRow({ row, org }: { row: RoleHeld; org: Org }) {
  const { t } = useTranslation();
  const subject = row.subject;
  const level = t(`roles.${row.role}`);
  const source = row.node.id === org.id ? t('access.roster.roleHere') : t('access.roster.inheritedFrom', { place: placeName(t, row.node) });
  return <li className="flex min-w-0 flex-col gap-2 py-2">
    <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-3 gap-y-1">
      <div className="min-w-0 flex-1">
        <SubjectView subject={subject} />
        <p className="text-xs text-text-muted wrap-anywhere">{source}</p>
      </div>
      <span className="text-sm text-text-muted">{level}</span>
    </div>
  </li>;
}

export function InheritedMembers({ org, members, query }: { org: Org; members: readonly Member[]; query: string }) {
  const { t } = useTranslation();
  const roles = useQuery(rolesQuery(org.id));
  const memberIds = new Set(members.map((member) => member.user.id));
  const ownerShown = memberIds.has(org.owner.id);
  const showOwner = !ownerShown && matches(query, org.owner.display_name);
  const rows = highest([...(roles.data?.above ?? []), ...(roles.data?.own.filter((row) => !row.domain) ?? [])])
    .filter((row) => row.subject.kind !== 'user' || (row.subject.id !== org.owner.id && !memberIds.has(row.subject.id)))
    .filter((row) => matches(query, row.subject.user?.display_name, row.subject.group?.name, row.subject.org?.name, row.subject.at?.name));
  if (!roles.isPending && !roles.isError && !showOwner && rows.length === 0) return null;
  return <section aria-label={t('access.roster.also')} className="flex flex-col gap-2">
    <h3 className="text-sm font-medium text-text">{t('access.roster.also')}</h3>
    {roles.isPending ? <SkeletonRows rows={2} /> : roles.isError ? <ErrorState compact error={roles.error} onRetry={() => void roles.refetch()} /> : <ul className="divide-y divide-border">
      {showOwner && <li className="flex min-w-0 items-start justify-between gap-3 py-2"><span className="font-medium text-text wrap-anywhere">{org.owner.display_name}</span><span className="text-sm text-text-muted">{t('access.roster.owner')}</span></li>}
      {rows.map((row) => <InheritedRow key={row.id} row={row} org={org} />)}
    </ul>}
  </section>;
}
