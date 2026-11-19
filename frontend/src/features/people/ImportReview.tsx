/** A dry run brings its choices, people, conflicts and omissions together before an import starts. */
import { useQuery } from '@tanstack/react-query';
import { Eye, FileText, Folder, MessageSquare } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { importKeys, previewImport } from '@/api/imports';
import { Hint } from '@/features/help/Hint';
import { ImportPeople } from '@/features/people/ImportPeople';
import { ImportReference } from '@/features/people/ImportReference';
import { ImportReasons } from '@/features/people/ImportReasons';
import { importReason } from '@/features/people/importReason';
import type { ImportAction, ImportOptions, ImportPlan, ImportPlanObject, ImportRequest, Node, PackageKind } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { useViewChoice } from '@/lib/viewChoices';
import { ByteSize } from '@/ui/ByteSize';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Markdown } from '@/ui/Markdown';
import { NodeIcon } from '@/ui/NodeIcon';
import { ImportPagination } from '@/features/people/ImportPagination';
import { Popover } from '@/ui/Popover';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Switch } from '@/ui/Switch';
import { Table, type Column } from '@/ui/Table';
import { Tabs } from '@/ui/Tabs';

type ReviewTab = 'content' | 'people' | 'conflicts' | 'notImported';
type KindFilter = 'all' | 'places' | 'folder' | 'document' | 'chat' | 'thread';
const REVIEW_TABS: readonly ReviewTab[] = ['content', 'people', 'conflicts', 'notImported'];
const KIND_FILTERS: readonly KindFilter[] = ['all', 'places', 'folder', 'document', 'chat', 'thread'];

function ObjectIcon({ kind }: { kind: PackageKind }) {
  if (kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread') return <NodeIcon kind={kind} className="size-4 shrink-0" />;
  const Icon = kind === 'folder' ? Folder : kind === 'chat' ? MessageSquare : FileText;
  return <Icon aria-hidden className="size-4 shrink-0 text-text-muted" />;
}

function Preview({ node, request, item }: { node: string; request: ImportRequest; item: ImportPlanObject }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const body = { source: request.source, object: item.source_id };
  const preview = useQuery({ queryKey: importKeys.preview(node, body), queryFn: ({ signal }) => previewImport(node, body, signal), enabled: open, retry: false, gcTime: 0 });
  const label = t('importWizard.review.previewFor', { name: item.name });
  return <Popover open={open} onOpenChange={setOpen} label={label} trigger={<IconButton size="sm" label={label} icon={<Eye aria-hidden />} />}>
    {preview.isPending ? <SkeletonRows rows={3} /> : preview.isError ? <ErrorAlert error={preview.error} action={<Button size="sm" onClick={() => void preview.refetch()}>{t('common.tryAgain')}</Button>} />
      : <div className="flex min-w-0 flex-col gap-2"><strong className="break-words">{preview.data.name}</strong>
        <Markdown text={preview.data.text} compact />{preview.data.truncated && <span className="text-sm text-text-muted">{t('importWizard.review.shortened')}</span>}
      </div>}
  </Popover>;
}

export function ImportReview({ node, request, plan, rows, hasMore, firstPage = true, onFirst = () => undefined, loadingMore, moreError, onMore, onOptions, onObject, onPerson }: {
  node: Node;
  request: ImportRequest;
  plan: ImportPlan;
  rows: readonly ImportPlanObject[];
  hasMore: boolean;
  firstPage?: boolean;
  onFirst?: () => void;
  loadingMore: boolean;
  moreError: unknown;
  onMore: () => void;
  onOptions: (patch: Partial<ImportOptions>) => void;
  onObject: (source: string, action: ImportAction) => void;
  onPerson: (principal: string, user: string | null) => void;
}) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [savedTab, rememberTab] = useViewChoice<string>('import-review', 'tab', 'content');
  const [savedKind, rememberKind] = useViewChoice<string>('import-review', 'kind', 'all');
  const [tab, setTab] = useState<ReviewTab>(() => REVIEW_TABS.includes(savedTab as ReviewTab) ? savedTab as ReviewTab : 'content');
  const [kind, setKind] = useState<KindFilter>(() => KIND_FILTERS.includes(savedKind as KindFilter) ? savedKind as KindFilter : 'all');
  const items = plan.counts.places + plan.counts.folders + plan.counts.documents + plan.counts.chats + plan.counts.threads;
  const packageSource = plan.source.mode === 'package';
  const sharingReason = !plan.access.carried ? t('importWizard.review.sharingNone') : !plan.access.allowed ? t('importWizard.review.sharingNeedsManage') : null;
  const reason = (value: { code: string; message: string }) => importReason(value, t, locale);
  const setSelectedTab = (next: ReviewTab) => { setTab(next); rememberTab(next); };
  const setSelectedKind = (next: KindFilter) => { setKind(next); rememberKind(next); };
  const shownRows = rows.filter((row) => (tab !== 'conflicts' || row.existing !== null || row.blocked !== null)
    && (kind === 'all' || (kind === 'places' ? row.kind === 'org' || row.kind === 'project' || row.kind === 'topic' : row.kind === kind)));
  const actionOptions = (allowed: readonly ImportAction[]) => allowed.map((action) => ({ value: action, label: t(`importWizard.review.${action}`) }));
  const columns: Column<ImportPlanObject>[] = [
    { id: 'name', header: t('importWizard.review.item'), cell: (item) => <div className="flex min-w-0 items-start gap-2 py-1">
      <ObjectIcon kind={item.kind} /><div className="min-w-0"><div className="truncate" title={item.name}>{item.name}</div>
        {item.path.length > 0 && <div className="truncate text-sm text-text-muted" title={item.path.join(' / ')}>{item.path.join(' / ')}</div>}
        {item.existing !== null && tab === 'conflicts' && <div className="text-sm text-text-muted">{t(item.existing.found_by === 'source' ? 'importWizard.review.sameSource' : 'importWizard.review.sameName')} · <ImportReference id={item.existing.id} kind={item.existing.kind} name={item.existing.name} /></div>}
        {item.blocked !== null && <div className="text-sm text-warning">{reason(item.blocked)}</div>}</div>
    </div> },
    { id: 'bytes', header: t('importWizard.review.size'), from: 'lg', align: 'end', cell: (item) => <ByteSize bytes={item.bytes} /> },
    { id: 'action', header: t('importWizard.review.action'), cell: (item) => item.blocked !== null ? null : <div className="flex flex-col gap-1">
      {item.allowed.length <= 1 ? <span className="text-sm text-text-muted">{t(`importWizard.review.${item.action}`)}</span> : <Select<ImportAction> size="sm" hideLabel label={t('importWizard.review.actionFor', { name: item.name })}
        value={item.action} options={actionOptions(item.allowed)} onValueChange={(action) => onObject(item.source_id, action)} fieldClassName="w-32 max-w-full" />}
      {item.action === 'skip' && ['org', 'project', 'topic', 'folder'].includes(item.kind) && <span className="max-w-32 whitespace-normal text-sm text-text-muted">{t('importWizard.review.skipContents')}</span>}
    </div> },
    { id: 'preview', header: t('importWizard.review.preview'), hideHeader: true, cell: (item) => <Preview node={node.id} request={request} item={item} /> },
  ];
  const defaultChoices = (value: ImportAction, change: (value: ImportAction) => void, label: string) => <Select<ImportAction> size="sm" label={label} value={value}
    onValueChange={change} options={actionOptions(['keep_both', 'replace', 'skip'])} fieldClassName="min-w-32" />;
  const table = <div className="flex min-w-0 flex-col gap-3">
    <div className="flex flex-wrap items-end gap-3">
      <Select<KindFilter> size="sm" label={t('importWizard.review.kind')} value={kind} onValueChange={setSelectedKind} options={[
        { value: 'all', label: t('importWizard.review.all') }, { value: 'places', label: t('importWizard.review.places') },
        { value: 'folder', label: t('importWizard.review.folders') }, { value: 'document', label: t('importWizard.review.documents') },
        { value: 'chat', label: t('importWizard.review.chats') }, { value: 'thread', label: t('importWizard.review.threads') },
      ]} />
      {tab === 'conflicts' && <>
        {defaultChoices(request.options.same_source ?? 'replace', (same_source) => onOptions({ same_source }), t('importWizard.review.sameSource'))}
        {defaultChoices(request.options.same_name ?? 'keep_both', (same_name) => onOptions({ same_name }), t('importWizard.review.sameName'))}
      </>}
    </div>
    {shownRows.length === 0 ? <StateLine compact message={t(hasMore || !firstPage ? 'importWizard.review.moreToCheck' : tab === 'conflicts' ? 'importWizard.review.noConflicts' : 'importWizard.review.nothing')} />
      : <Table key={`${tab}:${kind}`} label={t(tab === 'conflicts' ? 'importWizard.review.conflicts' : 'importWizard.review.content')} columns={columns} rows={shownRows} rowKey={(item) => item.source_id} boundedIdentity virtual />}
    <ImportPagination first={!firstPage} more={hasMore} loading={loadingMore} error={moreError} onMore={onMore} onFirst={onFirst} />
  </div>;
  return <div className="flex min-w-0 flex-col gap-4">
    <div className="flex flex-wrap items-center gap-2 text-sm text-text-muted tabular-nums">
      <span>{t('importWizard.review.summary', { count: items, items: formatNumber(items, locale), bytes: formatBytes(plan.source.bytes, locale), byteLimit: formatBytes(plan.limits.max_bytes, locale) })}</span>
      <Hint id="import-review" />
    </div>
    <ImportReasons reasons={plan.blockers} label={t('importWizard.dialog.blocked')} danger />
    <div className="flex flex-wrap items-start gap-x-6 gap-y-3">
      <Switch label={t('importWizard.review.comments')} checked={packageSource && (request.options.comments ?? true)} disabled={!packageSource} onCheckedChange={(comments) => onOptions({ comments })} />
      <Switch label={t('importWizard.review.versions')} checked={packageSource && (request.options.versions ?? true)} disabled={!packageSource} onCheckedChange={(versions) => onOptions({ versions })} />
      <Switch label={t('importWizard.review.sharing')} checked={request.options.access ?? false} disabled={sharingReason !== null} onCheckedChange={(access) => onOptions({ access })} />
    </div>
    {(!packageSource || sharingReason !== null) && <p className="text-sm text-text-muted">{[!packageSource ? t('importWizard.review.noHistory') : null, sharingReason].filter((one) => one !== null).join(' · ')}</p>}
    <Tabs<ReviewTab> label={t('importWizard.review.tabs')} value={tab} onValueChange={setSelectedTab} tabs={[
      { value: 'content', label: t('importWizard.review.content'), panel: table },
      { value: 'people', label: t('importWizard.review.people'), panel: <ImportPeople people={plan.people} authors={request.options.authors ?? plan.authors} mayAttribute={node.access.administers || node.access.role === 'admin'} onAuthors={(authors) => onOptions({ authors })} onPerson={onPerson} /> },
      { value: 'conflicts', label: t('importWizard.review.conflicts'), count: plan.conflicts, panel: table },
      { value: 'notImported', label: t('importWizard.review.notImported'), count: plan.losses.length, panel: plan.losses.length === 0 ? <StateLine compact message={t('importWizard.review.noLosses')} /> : <ImportReasons reasons={plan.losses} label={t('importWizard.review.notImported')} /> },
    ]} />
  </div>;
}
