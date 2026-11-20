import { createContext, useContext } from 'react';
import { ApiError } from '@/api/client';
import type { ResourceSharing } from '@/generated/api-types';
import { hasReadFailure, readData, useReadRecovery, type RecoverableRead } from '@/api/reads';
import { useViewShown } from '@/features/workbench/viewOpening';
import { SharingReadState } from '@/features/access/SharingReadState';
/** Home defaults, read from the same page and catalogue as the sharing dialog. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { defaultsQuery, permissionsQuery } from '@/api/access';
import { holderName, peopleOf } from '@/features/access/holders';
import { holderKey, indexCatalogue, linesOf, type Catalogue } from '@/features/access/model';
import type { Holder, NodeRef, PermissionPage } from '@/generated/api-types';
import { HolderChip } from '@/ui/HolderChip';
import { UserChip } from '@/ui/UserChip';

export const ListSharing = createContext<{
  homes: ReadonlyMap<string, ResourceSharing>;
  retry: (options?: { cancelRefetch?: boolean }) => unknown;
  read?: RecoverableRead;
  identity?: unknown;
} | null>(null);

interface PolicyLine { holder: Holder; verb: 'view' | 'interact' | 'manage' }

/** Mixed or profile-dependent policies keep their native details instead of claiming one audience. */
export function resourceSharingLines(page: Pick<PermissionPage, 'rows'>, catalogue: Catalogue, kinds: readonly string[]): PolicyLine[] | null {
  let same: PolicyLine[] | undefined;
  for (const kind of new Set(kinds)) {
    const rows = page.rows.filter((row) => catalogue.defs.get(row.permission)?.on.includes(kind) === true);
    if (rows.some((row) => row.profile !== 'any')) return null;
    const lines = linesOf({ ...page, rows }, catalogue, kind);
    if (lines.custom.length > 0) return null;
    const next = (['interact', 'view', 'manage'] as const).flatMap((verb) => lines[verb].map(({ holder }) => ({ holder, verb })));
    const key = (items: PolicyLine[]) => items.map(({ holder, verb }) => `${holderKey(holder)}:${verb}`).sort().join('|');
    if (same !== undefined && key(same) !== key(next)) return null;
    same = next;
  }
  return same ?? null;
}

export function ResourceSharingSummary({ home, kinds }: { home: NodeRef; kinds: readonly string[] }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const supplied = useContext(ListSharing);
  const defaultsOptions = defaultsQuery(home.id);
  const defaults = useQuery({ ...defaultsOptions, enabled: supplied === null && shown, retry: false });
  const catalogue = useQuery({ ...permissionsQuery, retry: false });
  const shared = supplied?.homes.get(home.id);
  const rows = shared?.rows, problem = shared?.problem;
  const sharingRead = supplied === null ? defaults : problem !== null && problem !== undefined
    ? { isError: true, error: new ApiError(problem, null), isFetching: supplied.read?.isFetching, refetch: supplied.retry }
    : supplied.read ?? { isError: false };
  useReadRecovery(sharingRead, shown, supplied === null ? defaultsOptions.queryKey : [supplied.identity, home.id]);
  useReadRecovery(catalogue, shown, permissionsQuery.queryKey);
  if (hasReadFailure(sharingRead)) return <SharingReadState reads={[sharingRead]} />;
  if (supplied !== null && (rows === null || rows === undefined)) {
    return null;
  }
  const page = supplied === null ? readData(defaults) : rows === null || rows === undefined ? undefined : { rows };
  const definitions = readData(catalogue);
  if (page === undefined || definitions === undefined) return <SharingReadState reads={supplied === null ? [defaults, catalogue] : [catalogue]} />;
  const lines = resourceSharingLines(page, indexCatalogue(definitions), kinds);
  const target = { kind: home.kind, id: home.id, name: home.name ?? t('access.place.hidden') };
  return <span data-resource-sharing className="flex min-w-0 flex-wrap items-center gap-x-1 text-sm text-text-muted">
    <span>{t(lines === null ? 'resourceGroups.policyCustom' : lines.length === 0 ? 'resourceGroups.policyNone' : 'resourceGroups.policyDefaults')}</span>
    {lines?.map(({ holder, verb }, at) => <span key={`${holderKey(holder)}:${verb}`} className="inline-flex min-w-0 max-w-full items-center gap-1">
      {at > 0 && <span aria-hidden>·</span>}
      {holder.kind === 'user' ? <UserChip user={holder.user} size={24} />
        : <HolderChip bare name={holderName(t, holder, target.name, true)} people={peopleOf(holder, target)} />}
      <span>{t(`resourceGroups.policyVerbs.${verb}`)}</span>
    </span>)}
  </span>;
}
