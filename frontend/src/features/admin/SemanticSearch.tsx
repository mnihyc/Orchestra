/** Search by meaning: what the system names for it, and each root org's index with its state, progress and estimate, the disk it takes and why it waits.
 *
 * The keys `search.semantic.*` are edited here at the system with the editor every place uses; an org changes
 * them on its own settings. While an index is being made the page reads itself again at the indexer's own
 * interval. An org where nothing is named can be given the provider Local embeddings, which is then named
 * there; an index that waits for a choice offers the two, with the size and time of a new set shown first.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useMemo,useState } from 'react';
import {useStore} from 'zustand';
import {client} from '@/api/client';
import {SemanticActionsOwner,useSemanticActions} from './semanticActions';
import {useCollectionSearch} from '@/lib/collectionSearch';
import {CollectionSearch} from '@/ui/CollectionSearch';
import {Pagination} from '@/ui/Pagination';
import { ReferenceCard } from '@/ui/ReferenceCard';
import { PLACE_ICONS } from '@/features/navigation/places';
import { isPlainClick } from '@/features/chat/open';
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { semanticSearchPageQuery } from '@/api/admin';
import { StatusDot, type DotTone } from '@/ui/StatusDot';
import { providerQuery, templateQuery } from '@/api/resources';
import { SettingsEditor } from '@/features/settings/Editor';
import { Hint } from '@/features/help/Hint';
import type { SemanticSearchOrg, SemanticState } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber, formatTokens } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Meter } from '@/ui/Meter';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tooltip } from '@/ui/Tooltip';
import { type Column, Table } from '@/ui/Table';

/** The keys of search by meaning, edited here at the system. */
const KEYS = ['search.semantic.enabled', 'search.semantic.embeddings', 'search.semantic.max_disk_share', 'search.semantic.tuning'];

const MODEL_COLUMN = 'model';
const PROGRESS_COLUMN = 'progress';
const DISK_COLUMN = 'disk';
const STATE_COLUMN = 'state';
const ACTIONS_COLUMN = 'actions';

const TONES: Record<SemanticState, DotTone> = {
  off: 'off',
  not_named: 'idle',
  unavailable: 'danger',
  idle: 'accent',
  indexing: 'accent',
  paused: 'warning',
  waiting: 'warning',
};

/** The reasons the server gives, by word; another word is shown as it comes. */
const REASONS = ['disk_share', 'provider', 'provider_disabled', 'provider_removed', 'model', 'access', 'cap', 'machine', 'extension', 'choice', 'error'] as const;

function providerPaused(org: SemanticSearchOrg): boolean {
  return org.state === 'paused' && (org.reason === 'provider_disabled' || org.reason === 'provider_removed');
}

function isReason(word: string): word is (typeof REASONS)[number] {
  return (REASONS as readonly string[]).includes(word);
}

/** The model named and the length asked of it, as one label. */
function modelLabel(org: SemanticSearchOrg, count: (value: number) => string): string {
  const parts = [org.model_name ?? '', org.dimensions === null ? '' : count(org.dimensions)];
  return parts.filter((part) => part !== '').join(' · ');
}

/** How far an org's index got: the share of its tokens that have their vectors. */
function progressOf(org: SemanticSearchOrg): number {
  return org.tokens > 0 ? ((org.tokens - org.pending_tokens) / org.tokens) * 100 : 0;
}

function useFormats() {
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  return {
    bytes: (value: number) => formatBytes(value, locale),
    count: (value: number) => formatNumber(value, locale),
    tokens: (value: number) => formatTokens(value, locale),
    span: (seconds: number) => formatSpan(seconds * 1000, { locale, timeZone }),
  };
}

function State({ org }: { org: SemanticSearchOrg }) {
  const { t } = useTranslation();
  const [open,setOpen]=useState(false);
  const reason = org.state === 'indexing' || providerPaused(org) ? null : (org.reason ?? null);
  const state = <span className="flex min-w-0 flex-wrap items-center gap-2">
    <StatusDot tone={providerPaused(org) ? 'idle' : TONES[org.state]} label={t(`admin.semantic.states.${org.state}`)} showLabel />
    {reason !== null && <span className="text-sm text-text-muted">· {isReason(reason) ? t(`admin.semantic.reasons.${reason}`) : reason}</span>}
  </span>;
  return org.detail === null || providerPaused(org) ? state : <Tooltip content={org.detail} open={open} onOpenChange={setOpen}>
    <button type="button" className="focus-inset min-w-0 rounded-sm text-left" onClick={()=>setOpen((before)=>!before)}>{state}</button>
  </Tooltip>;
}

function Progress({ org, inline = false }: { org: SemanticSearchOrg; inline?: boolean }) {
  const { t } = useTranslation();
  const format = useFormats();
  if (providerPaused(org) && org.reason !== null && isReason(org.reason)) return <span className="flex min-w-0 flex-col gap-1">
    {org.tokens > 0 && <Meter value={progressOf(org)} label={t('admin.semantic.progress')} look="compact" neutral />}
    <span className="text-sm text-text-muted">{t(`admin.semantic.reasons.${org.reason}`)}</span>
  </span>;
  if(org.state==='indexing' && (org.reason==='counting'||org.tokens===0)) return <span className="text-sm text-text-muted tabular-nums">{t('admin.semantic.counting',{tokens:format.tokens(org.tokens)})}</span>;
  if (org.tokens === 0) return <span className="text-text-muted">—</span>;
  const detail =
    org.pending_tokens === 0
      ? t('admin.semantic.done')
      : org.seconds_left === null
        ? t('admin.semantic.pending', { tokens: format.tokens(org.pending_tokens) })
        : t('admin.semantic.left', { time: format.span(org.seconds_left) });
  return (
    <span className={inline ? 'flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1' : 'flex min-w-0 flex-col gap-1'}>
      <Meter value={progressOf(org)} label={t('admin.semantic.progress')} look="compact" neutral />
      <span className="text-sm text-text-muted">{detail}</span>
    </span>
  );
}

function Disk({ org }: { org: SemanticSearchOrg }) {
  const { t } = useTranslation();
  const format = useFormats();
  return <span className="tabular-nums">{org.allowed_bytes === null
    ? t('admin.semantic.diskUsed', { used: `~${format.bytes(org.used_bytes)}` })
    : t('admin.semantic.diskOf', { used: `~${format.bytes(org.used_bytes)}`, allowed: format.bytes(org.allowed_bytes) })}</span>;
}

function Actions({ org }: { org: SemanticSearchOrg }) {
  const { t } = useTranslation();
  const format = useFormats();
  const {setAdding,setSettings,rebuild,keep}=useSemanticActions(org);
  if (org.state === 'not_named') {
    return (
      <span className="flex flex-col items-start gap-1">
        <Button size="sm" onClick={() => setAdding(true)}>
          {t('admin.semantic.addLocal')}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setSettings(true)}>{t('admin.semantic.embeddings')}</Button>
      </span>
    );
  }
  if (org.state !== 'waiting') return null;
  return (
    <span className="flex flex-col items-start gap-1">
      {org.rebuild_bytes !== null && (
        <span className="text-sm text-text-muted">
          {org.rebuild_seconds === null
            ? t('admin.semantic.newSet', { size: format.bytes(org.rebuild_bytes) })
            : t('admin.semantic.newSetTime', { size: format.bytes(org.rebuild_bytes), time: format.span(org.rebuild_seconds) })}
        </span>
      )}
      <span className="flex flex-wrap gap-2">
        <Button size="sm" pending={rebuild.isPending} disabled={keep.isPending} onClick={() => rebuild.mutate()}>
          {t('admin.semantic.rebuild')}
        </Button>
        <Button size="sm" variant="ghost" pending={keep.isPending} disabled={rebuild.isPending} onClick={() => keep.mutate()}>
          {t('admin.semantic.keep')}
        </Button>
      </span>
    </span>
  );
}

function OrgName({org}: {org: SemanticSearchOrg['org']}) {
  const Icon = PLACE_ICONS.org;
  const name = org.name ?? '';
  return <ReferenceCard reference={{kind: 'node', id: org.id}} label={name}>{(keep) => <a href={`/org/${org.id}`}
    onClick={(event) => {if (isPlainClick(event)) {event.preventDefault(); keep();}}}
    className="inline-flex max-w-full min-w-0 items-center gap-1 rounded-sm bg-bg-muted px-1 font-medium text-text hover:underline">
    <Icon aria-hidden className="size-3.5 shrink-0" /><span className="truncate">{name}</span>
  </a>}</ReferenceCard>;
}

function TemplateUsed({ providerId }: { providerId: string | null }) {
  const {t} = useTranslation();
  const provider = useQuery({...providerQuery(providerId ?? ''), enabled: providerId !== null, retry: false});
  const config = provider.data?.runtime_config;
  const id = config !== null && typeof config === 'object' && !Array.isArray(config) && 'runtime.template_id' in config ? config['runtime.template_id'] : undefined;
  const template = useQuery({...templateQuery(typeof id === 'string' ? id : ''), enabled: typeof id === 'string', retry: false});
  if (template.data === undefined) return null;
  return <Link to="/resources/templates/$id" params={{id: template.data.id}} className="truncate text-sm text-text-muted hover:underline">{t('admin.semantic.template', {name: template.data.name})}</Link>;
}

export function SemanticSearch() {
 const reader=useStore(client.session,(state)=>`${state.userId}:${state.readerVersion}`);
 return <SemanticActionsOwner key={reader}><SemanticSearchPage/></SemanticActionsOwner>;
}
function SemanticSearchPage() {
  const { t } = useTranslation();
  const format = useFormats();
  // Follow the indexer's interval even while idle: a changed setting can begin work after this read.
  const search=useCollectionSearch('admin-semantic-orgs');
  const overview = useInfiniteQuery({
    ...semanticSearchPageQuery(search.q),
    refetchInterval: (query) => query.state.data?.pages[0]===undefined ? false : query.state.data.pages[0].interval_secs * 1000,
  });
  const rows=useMemo(()=>overview.data?.pages.flatMap((page)=>page.orgs)??[],[overview.data]);

  if (overview.isPending) return <SkeletonRows read={overview} rows={4} />;
  if (overview.isError&&!overview.isFetchNextPageError) return <ErrorState error={overview.error} retrying={overview.isRefetching} onRetry={() => void overview.refetch()} />;

  const data = overview.data?.pages[0];
  if(data===undefined)return <SkeletonRows rows={4}/>;
  const columns: Column<SemanticSearchOrg>[] = [
    { id: 'org', header: t('admin.semantic.org'), grow: true, cell: (row, shown) => <span className="flex min-w-0 flex-col gap-1">
      <span className="flex flex-wrap items-center gap-2"><OrgName org={row.org} />
        {!shown.has(STATE_COLUMN) && <State org={row} />}</span>
      <TemplateUsed providerId={row.provider_id} />
      {!shown.has(MODEL_COLUMN) && row.model_name !== null && <span className="break-all text-sm text-text-muted">{modelLabel(row, format.count)}</span>}
      {!shown.has(PROGRESS_COLUMN) && (row.tokens > 0 || providerPaused(row) || row.state === 'indexing') && <Progress org={row} inline />}
      {!shown.has(DISK_COLUMN) && row.used_bytes > 0 && <span className="text-sm text-text-muted"><Disk org={row} /></span>}
      {!shown.has(ACTIONS_COLUMN) && <Actions org={row} />}
    </span> },
    { id: STATE_COLUMN, header: t('admin.semantic.state'), from: 'sm', className: 'whitespace-normal', cell: (row) => <State org={row} /> },
    {
      id: 'model',
      header: t('admin.semantic.model'),
      from: 'md',
      className: 'whitespace-normal',
      cell: (row) =>
        row.model_name === null ? (
          <span className="text-text-muted">—</span>
        ) : (
          <span className="break-all">{modelLabel(row, format.count)}</span>
        ),
    },
    { id: PROGRESS_COLUMN, header: t('admin.semantic.progress'), from: 'md', cell: (row) => <Progress org={row} /> },
    {
      id: 'disk',
      header: t('admin.semantic.disk'),
      align: 'end',
      from: 'lg',
      cell: (row) => <Disk org={row} />,
    },
    { id: ACTIONS_COLUMN, header: t('admin.semantic.actions'), from: 'lg', className: 'whitespace-normal', cell: (row) => <Actions org={row} /> },
  ];

  return (
    <div className="flex flex-col gap-6">
      {!data.available && (
        <InlineAlert tone="warning" title={t('admin.semantic.unavailable')}>
          {t(data.unavailable_reason === 'not_permitted' ? 'admin.semantic.notPermitted' : 'admin.semantic.absent')}
        </InlineAlert>
      )}
      <Section id="admin-semantic-orgs" title={t('admin.semantic.orgsTitle')} action={<Hint id="semantic-search-index" />}>
        <CollectionSearch name={t('admin.semantic.orgsTitle')} {...search}/>
        {rows.length === 0 ? (
          <p className="text-sm text-text-muted">{t('admin.semantic.noOrgs')}</p>
        ) : (
          <Table label={t('admin.semantic.orgsTitle')} columns={columns} rows={rows} virtual rowKey={(row) => row.org.id} />
        )}
        <Pagination hasMore={overview.hasNextPage} loading={overview.isFetchingNextPage} error={overview.isFetchNextPageError?overview.error:null} onMore={()=>{void overview.fetchNextPage();}}/>
      </Section>
      <Section id="admin-semantic-choice" title={t('admin.semantic.choiceTitle')}>
        <SettingsEditor target={{ kind: 'system' }} scope="system" only={KEYS} compact place={{ name: t('admin.nav.groups.system'), owner: null, access: null, kind: null }} />
      </Section>
    </div>
  );
}
