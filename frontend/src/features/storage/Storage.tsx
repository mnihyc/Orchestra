/** One retained-storage report reuses Usage's scope, figures, breakdowns and records frame. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Filter, HardDrive, RefreshCw } from 'lucide-react';
import { useEffect, useState } from 'react';
import { ApiError } from '@/api/client';
import { useTranslation } from 'react-i18next';
import { storagePagesQuery } from '@/api/storage';
import { useAdminMode, useManagementAvailable, AdminModeEnableAction } from '@/features/access/AdminMode';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { KnownNodeChip } from '@/features/people/NamedChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { MachineLink } from '@/features/resources/machines/MachineLink';
import { resourceSubjectOf } from '@/features/resource-facts/facts';
import { ScopePicker } from '@/features/usage/ScopePicker';
import { ReportBreakdowns, ReportFigure, ReportFrame, ReportRecords } from '@/features/usage/ReportFrame';
import type { KeptRecord, KeptWhy, StorageBreakdownRow, StorageDimension, StorageKind, StorageTotals, VisibleDependent } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { StatusDot } from '@/ui/StatusDot';
import { Table, type Column } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';
import { UserChip } from '@/ui/UserChip';
import { STORAGE_DIMENSIONS, STORAGE_KINDS, STORAGE_WHYS, storageRequest, type StoragePatch, type StorageSearch } from './search';

/** Radix reserves an empty value for no selection; All is a real local choice. */
const ALL = '\u0000all';
const COLUMN = {why:'why',bytes:'bytes',owner:'owner',where:'where',dependents:'dependents',observed:'observed'} as const;

function Reason({ row }: { row: KeptRecord }) {
  const { t } = useTranslation();
  return <div className="flex flex-col gap-1"><StatusDot showLabel label={t(`storage.whys.${row.why}`)} tone={row.why === 'unknown' ? 'warning' : 'idle'} />{row.until !== null && <RelativeTime at={row.until} className="text-sm text-text-muted" />}{row.blocker !== null && <span className="text-sm text-text-muted">{t(`storage.blockers.${row.blocker}`,{defaultValue:row.blocker})}</span>}</div>;
}

export function DependentName({ row }: { row: Pick<VisibleDependent, 'object' | 'name'> }) {
  const object = row.object;
  if (object === null) return <span className="min-w-0 truncate" title={row.name}>{row.name}</span>;
  const subject = resourceSubjectOf(object, row.name);
  if (subject !== null) return <ResourceChip subject={subject} hint={false} />;
  if (object.kind === 'thread' || object.kind === 'org' || object.kind === 'project' || object.kind === 'topic') return <KnownNodeChip node={{ id: object.id, kind: object.kind, name: row.name, redacted: false }} />;
  return <span className="min-w-0 truncate" title={row.name}>{row.name}</span>;
}

/** Logical retained data names its resource; physical artifacts keep their own inventory name. */
export function RecordName({ row }: { row: KeptRecord }) {
  const { t } = useTranslation();
  const name = row.name || t(`storage.kinds.${row.kind}`);
  const subject = row.resource !== null && (row.kind === 'environment_files' || row.kind === 'mount_data') ? resourceSubjectOf(row.resource, name) : null;
  return subject !== null ? <ResourceChip subject={subject} hint={false} /> : <Tooltip content={name}><span className="truncate font-medium">{name}</span></Tooltip>;
}

/** Logical place and physical host are independently permission-filtered by the report. */
function Where({ row }: { row: KeptRecord }) {
  const { t } = useTranslation();
  const server = ['database','content','upload','export','release','dump'].includes(row.kind);
  if (row.place === null && row.host === null && row.machine === null) return <span className="text-sm text-text-muted">{server ? t('storage.server') : '—'}</span>;
  return <div className="flex min-w-0 flex-col items-start gap-1">
    {row.place !== null && <KnownNodeChip node={row.place} />}
    {row.host !== null ? <ResourceChip subject={{kind:'host',host:row.host}} hint={false} /> : row.machine !== null ? <MachineLink machine={row.machine} /> : null}
  </div>;
}

export function Dependents({ row }: { row: KeptRecord }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (row.facts === null) return <Tooltip content={t('storage.notRecorded')}><span className="text-text-muted">—</span></Tooltip>;
  const groups = row.facts.dependents;
  if (groups.length === 0 && row.facts.dependents_complete) return <span className="text-sm text-text-muted">{t('storage.noDependents')}</span>;
  return <Popover label={t('storage.keptBy')} trigger={<Button size="sm" variant="ghost">{t('storage.keptBy')}</Button>} className="w-[20rem]">
    <div className="flex flex-col gap-3">{groups.map((group) => <div key={group.relation} className="flex min-w-0 flex-col gap-1">
      <span className="text-sm text-text-muted">{t(`storage.relations.${group.relation}`, { number: formatNumber(group.count, locale) })}</span>
      {group.visible.map((dependent, index) => <DependentName key={`${dependent.object?.id ?? 'system'}:${index}`} row={dependent} />)}
      {group.hidden > 0 && <span className="text-sm text-text-muted">{t('storage.hidden', { number: formatNumber(group.hidden, locale) })}</span>}
    </div>)}{!row.facts.dependents_complete && <span className="text-sm text-text-muted">{t('storage.notRecorded')}</span>}</div>
  </Popover>;
}

function sizeOf(totals: StorageTotals, locale: string, incomplete = false): string {
  if ((totals.unknown > 0 || incomplete) && totals.bytes === 0) return '—';
  return `${totals.unknown > 0 || incomplete ? '≥' : ''}${formatBytes(totals.bytes, locale)}`;
}

export function Storage({ search, onSearch, onCleanup, onScopeUnavailable, onReportReady }: {
  search: StorageSearch;
  onSearch: (patch: StoragePatch) => void;
  onCleanup?: (ids: string[]) => void;
  onScopeUnavailable?: () => void;
  onReportReady?: () => void;
}) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const canAdminister = useManagementAvailable();
  const adminMode = useAdminMode();
  const selectionKey=JSON.stringify(storageRequest(search));
  const [choice,setChoice]=useState<{key:string;ids:Set<string>}>({key:selectionKey,ids:new Set()});
  const selected=choice.key===selectionKey ? choice.ids : new Set<string>();
  const setSelected=(value:Set<string>|((before:Set<string>)=>Set<string>))=>setChoice((before)=>({key:selectionKey,ids:typeof value==='function' ? value(before.key===selectionKey ? before.ids : new Set()) : value}));
  const scope = search.scope ?? 'me';
  const scopeBlocked = scope === 'system' && adminMode.data?.as_admin !== true;
  const query = useInfiniteQuery({ ...storagePagesQuery(storageRequest(search)), enabled: !scopeBlocked });
  const data = query.data?.pages[0];
  useEffect(() => { if (query.isSuccess && !scopeBlocked) onReportReady?.(); }, [query.isSuccess, scopeBlocked, onReportReady]);
  useEffect(() => {
    if (scopeBlocked && adminMode.isSuccess || query.error instanceof ApiError && (query.error.is('not_found') || query.error.is('authz.denied'))) onScopeUnavailable?.();
  }, [query.error, scopeBlocked, adminMode.isSuccess, onScopeUnavailable]);
  const by = search.by ?? 'kind';
  useScreenHeading({ title: t('storage.title'), tabTitle: t('storage.title'), icon: HardDrive });
  const scopeKind = scope.startsWith('node:') ? 'node' : scope.startsWith('host:') ? 'host' : scope === 'system' ? 'system' : 'me';
  const scopeName = (data?.name === '' ? undefined : data?.name) ?? (scope === 'system' ? t('storage.system') : data?.items.find((row) => row.host !== null)?.host?.name ?? data?.unavailable.find((row) => row.host !== null)?.host?.name ?? null);
  const filters = <>
    <ScopePicker scope={{kind:scopeKind}} name={scopeName} kinds={['org', 'project', 'host']} mineLabel={t('storage.mine')} placeholder={t('storage.scopeSearch')} nothing={t('storage.scopeEmpty')} onChange={(next) => onSearch({scope:next === 'me' ? undefined : next})} />
    {canAdminister && <Button size="sm" variant="ghost" onClick={() => onSearch({scope:scope === 'system' ? undefined : 'system'})}>{t(scope === 'system' ? 'storage.mine' : 'storage.system')}</Button>}
    <Select<StorageKind | typeof ALL> size="sm" label={t('storage.kind')} value={search.kind ?? ALL} onValueChange={(kind) => onSearch({kind:kind === ALL ? undefined : kind})} options={[{value:ALL,name:t('storage.allKinds')},...STORAGE_KINDS.map((kind) => ({value:kind,name:t(`storage.kinds.${kind}`)}))]} />
    <Select<KeptWhy | typeof ALL> size="sm" label={t('storage.why')} value={search.why ?? ALL} onValueChange={(why) => onSearch({why:why === ALL ? undefined : why})} options={[{value:ALL,name:t('storage.allReasons')},...STORAGE_WHYS.map((why) => ({value:why,name:t(`storage.whys.${why}`)}))]} />
    {(search.kind !== undefined || search.why !== undefined || search.age !== undefined || search.host !== undefined || search.place !== undefined || search.owner !== undefined || search.resource_id !== undefined) && <Button size="sm" variant="ghost" onClick={() => onSearch({kind:undefined,why:undefined,age:undefined,host:undefined,place:undefined,owner:undefined,resource_id:undefined,resource_kind:undefined})}>{t('storage.clearFilters')}</Button>}
    <IconButton size="sm" icon={<RefreshCw aria-hidden />} label={t('storage.refresh')} onClick={() => void query.refetch()} />
  </>;
  const rows = query.data?.pages.flatMap((page) => page.items) ?? [];
  const columns: Column<KeptRecord>[] = [
    ...(onCleanup === undefined ? [] : [{id:'selected',header:<Checkbox label={<span className="sr-only">{t('storage.selectAll')}</span>} checked={rows.length > 0 && rows.every((row) => selected.has(row.id))} onCheckedChange={(value) => setSelected(value ? new Set(rows.map((row) => row.id)) : new Set())} />,className:'w-9',cell:(row:KeptRecord) => <Checkbox label={<span className="sr-only">{t('storage.select', {name:row.name || t(`storage.kinds.${row.kind}`)})}</span>} checked={selected.has(row.id)} onCheckedChange={(value) => setSelected((before) => {const next=new Set(before);if(value)next.add(row.id);else next.delete(row.id);return next;})} />}]),
    {id:'name',header:t('storage.name'),grow:true,cell:(row,shown) => <div className="flex min-w-0 flex-col gap-1"><RecordName row={row} /><span className="text-sm text-text-muted">{t(`storage.kinds.${row.kind}`)}</span>{!shown.has(COLUMN.why) && <Reason row={row} />}{!shown.has(COLUMN.bytes) && <span className="text-sm text-text-muted">{row.bytes === null ? '—' : formatBytes(row.bytes,locale)}</span>}{!shown.has(COLUMN.owner) && row.owner !== null && <UserChip user={row.owner} />}{!shown.has(COLUMN.where) && <Where row={row} />}{!shown.has(COLUMN.dependents) && <Dependents row={row} />}{!shown.has(COLUMN.observed) && <RelativeTime at={row.observed_at} className="text-sm text-text-muted" />}</div>},
    {id:'why',header:t('storage.why'),from:'sm',cell:(row) => <Reason row={row} />},
    {id:'bytes',header:t('storage.size'),from:'sm',align:'end',cell:(row) => row.bytes === null ? <Tooltip content={t('storage.notMeasured')}><span>—</span></Tooltip> : <span className="tabular-nums">{formatBytes(row.bytes,locale)}</span>},
    {id:'where',header:t('storage.where'),from:'lg',cell:(row) => <Where row={row} />},
    {id:'owner',header:t('storage.owner'),from:'lg',cell:(row) => row.owner === null ? <span className="text-sm text-text-muted">—</span> : <UserChip user={row.owner} />},
    {id:'dependents',header:t('storage.keptBy'),from:'md',cell:(row) => <Dependents row={row} />},
    {id:'observed',header:t('storage.observed'),from:'lg',cell:(row) => <RelativeTime at={row.observed_at} className="text-sm text-text-muted" />},
  ];
  const table = (dimension: StorageDimension) => {
    const rows = data?.breakdowns.find((table) => table.by === dimension)?.rows ?? [];
    const filter = (row: StorageBreakdownRow) => {
      if (dimension === 'host' || dimension === 'place' || dimension === 'owner') {
        onSearch({[dimension]:row.key});
      } else if (dimension === 'kind') onSearch({kind:STORAGE_KINDS.find((kind) => kind === row.key)});
      else if (dimension === 'why') onSearch({why:STORAGE_WHYS.find((why) => why === row.key)});
      else onSearch({age:row.key});
    };
    return <Table label={t(`storage.dimensions.${dimension}`)} rows={rows} rowKey={(row) => row.key} columns={[
      {id:'member',header:t(`storage.dimensions.${dimension}`),grow:true,cell:(row) => row.owner !== null || row.object !== null
        ? <div className="flex min-w-0 items-center gap-1">{row.owner !== null ? <UserChip user={row.owner} /> : <DependentName row={{object:row.object,name:row.name}} />}<IconButton size="sm" icon={<Filter aria-hidden />} label={t('storage.filter')} onClick={() => filter(row)} /></div>
        : <Button size="sm" variant="ghost" onClick={() => filter(row)}>{dimension === 'kind' ? t(`storage.kinds.${row.key}`,{defaultValue:row.name}) : dimension === 'why' ? t(`storage.whys.${row.key}`,{defaultValue:row.name}) : dimension === 'age' ? t(`storage.ages.${row.key}`,{defaultValue:row.name}) : row.key === 'server' ? t('storage.server') : row.key === 'unknown' ? t('storage.unknown') : row.name}</Button>},
      {id:'items',header:t('storage.records'),align:'end',cell:(row) => formatNumber(row.totals.items,locale)},
      {id:'bytes',header:t('storage.size'),align:'end',cell:(row) => sizeOf(row.totals,locale)},
    ]} />;
  };
  return <ReportFrame title={t('storage.title')} icon={<HardDrive aria-hidden />} filters={filters} actions={<ArticleGuideLink article="storage-cleanup" heading="storage-report" title={t('storage.guide')} />}>
    {scopeBlocked ? <StateLine title={t('storage.adminOnly')} action={<AdminModeEnableAction />} /> : null}
    {scopeBlocked ? null : query.isPending ? <SkeletonRows rows={6} /> : query.isError && data === undefined ? <ErrorState error={query.error} onRetry={() => void query.refetch()} /> : data === undefined ? null : <>
      {data.unavailable.map((source,index) => <StateLine key={`${source.host?.id ?? 'server'}:${index}`} title={source.host?.name ?? t('storage.system')} message={t(`storage.sources.${source.reason}`,{defaultValue:source.reason})} />)}
      <ul aria-label={t('storage.figures')} className="grid grid-cols-1 gap-3 sm:grid-cols-3 xl:grid-cols-5"><ReportFigure label={t('storage.totalKept')} value={sizeOf(data.totals,locale,data.unavailable.length > 0)} />{STORAGE_WHYS.map((why) => <ReportFigure key={why} label={t(`storage.whys.${why}`)} value={sizeOf(data.breakdowns.find((table)=>table.by==='why')?.rows.find((row)=>row.key===why)?.totals ?? {items:0,bytes:0,unknown:0},locale,data.unavailable.length > 0)} />)}</ul>
      <ReportBreakdowns label={t('storage.breakdowns')} value={by} onValueChange={(by) => onSearch({by})} tabs={STORAGE_DIMENSIONS.map((dimension) => ({value:dimension,label:t(`storage.dimensions.${dimension}`),panel:table(dimension)}))} />
      <ReportRecords id="storage-records" title={t('storage.records')} about={<span className="text-sm text-text-muted">{t('storage.recordSummary',{records:formatNumber(data.totals.items,locale),unmeasured:formatNumber(data.totals.unknown,locale)})}</span>} actions={onCleanup === undefined ? undefined : <Button size="sm" disabled={selected.size === 0} onClick={() => onCleanup([...selected])}>{t('storage.clean')}</Button>}>
        {rows.length === 0 ? <EmptyState compact icon={<HardDrive aria-hidden />} title={t('storage.empty')} /> : <Table boundedIdentity label={t('storage.records')} columns={columns} rows={rows} rowKey={(row) => row.id} />}
        <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : undefined} onMore={() => void query.fetchNextPage()} />
      </ReportRecords>
    </>}
  </ReportFrame>;
}
