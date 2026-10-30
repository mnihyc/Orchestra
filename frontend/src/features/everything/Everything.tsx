/** Everything the signed-in person may see: one shared tree-grid page for personal and system scopes. */
import { useIsFetching, useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowDownUp, Columns3, ListTree, ListChecks, RefreshCw, Share2, SlidersHorizontal } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { meQuery } from '@/api/auth';
import { useAdminMode } from '@/features/access/AdminMode';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import type { EverythingFilter } from '@/api/everything';
import { namesQuery } from '@/api/search';
import { COLUMN_SORT, OPTIONAL_COLUMNS, readColumns, saveColumns, type Column, type OptionalColumn } from '@/features/everything/columns';
import { EverythingTable } from '@/features/everything/EverythingTable';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { EVERYTHING_KINDS, initialView, validView, type EverythingView } from './view';
import { ArrangementSwitch, RecentOrderSelect } from '@/features/navigation/ArrangementSwitch';
import { writeViewChoice } from '@/lib/viewChoices';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { openAccessToMany } from '@/features/access/AccessDialog';
import { useHelpOpener } from '@/features/help/open';
import type { EverythingKind, EverythingRow, EverythingScope, Me } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { Checkbox } from '@/ui/Checkbox';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { DateField } from '@/ui/DateField';
import { FilterBar } from '@/ui/FilterBar';
import { PageHeader } from '@/shell/Page';
import { Menu } from '@/ui/Menu';
import { Popover } from '@/ui/Popover';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { choiceOption } from '@/i18n/choices';

const KINDS = EVERYTHING_KINDS;
const SORT_COLUMNS = ['name', 'where', 'opened', 'activity_by', ...OPTIONAL_COLUMNS.filter((column) => column !== 'admin_access')] as const;
const RESOURCE_GUIDES: Partial<Record<EverythingKind, string>> = {
  environment: 'environments-and-their-doors', provider: 'providers-and-models', machine: 'profiles-hosts-and-computers',
  host: 'profiles-hosts-and-computers', mount: 'mounts-and-tool-servers', template: 'templates-and-updates',
  agent: 'profiles-hosts-and-computers', mcp_server: 'mounts-and-tool-servers',
};

interface OwnerChoice { id: string; name: string }

/** A person lookup needs no org-tree inventory merely to label its results. */
function OwnerFilter({ owner, onChoose }: { owner: OwnerChoice | null; onChoose: (owner: OwnerChoice | null) => void }) {
  const { t } = useTranslation();
  const selected = owner?.id ?? null;
  const [draft, setDraft] = useState({ selected, text: owner?.name ?? '' });
  if (draft.selected !== selected) setDraft({ selected, text: owner?.name ?? '' });
  const typed = draft.selected === selected ? draft.text : owner?.name ?? '';
  const words = useDebounced(typed.trim());
  const names = useQuery({ ...namesQuery({ q: words, kinds: ['user'] }), enabled: typed.trim() !== '' && words !== '' && words !== owner?.name });
  const hits = names.data ?? [];
  return <div className="flex min-w-0 flex-col gap-1">
    <Combobox label={t('everything.columns.owner')} query={typed} placeholder={t('everything.findOwner')}
      onQueryChange={(text) => { setDraft({ selected: null, text }); if (owner !== null) onChoose(null); }} loading={names.isFetching}
      empty={t('everything.noOwnerMatches')} minChars={1}
      groups={[{ id: 'people', label: t('everything.columns.owner'), options: hits.map((hit) => ({ value: hit.id,
        label: t('inventoryChoices.everything.owner.name', { name: hit.name }), description: t('inventoryChoices.everything.owner.description', { person: hit.username === null ? hit.name : `@${hit.username}` }), text: hit.name })) }]}
      onSelect={(option) => { const hit = hits.find((hit) => hit.id === option.value); if (hit !== undefined) { setDraft({ selected: hit.id, text: hit.name }); onChoose({ id: hit.id, name: hit.name }); } }} />
    {owner !== null && <Button size="sm" variant="ghost" onClick={() => { setDraft({ selected: null, text: '' }); onChoose(null); }}>{t('everything.anyOwner')}</Button>}
  </div>;
}

export interface EverythingBrowserProps {
  view?: Partial<EverythingView> | undefined;
  onView?: ((view: EverythingView) => void) | undefined;
  scope?: EverythingScope;
  parent?: string | null;
  q?: string;
  participating?: boolean;
  onParticipation?: (value: boolean) => void;
  onQuery?: ((value: string) => void) | undefined;
  onScope?: ((scope: EverythingScope) => void) | undefined;
}

function Browser({ person, scope = 'mine', parent = null, q = '', onQuery, onScope, participating: focus, onParticipation, view: explicit, onView }: EverythingBrowserProps & { person: Me }) {
  const { t } = useTranslation();
  const openGuide = useHelpOpener();
  const help = (name: string, article: string, heading?: string) => ({ label: t('inventoryChoices.help.open', { name }), onOpen: () => openGuide(article, heading) });
  const client = useQueryClient();
  const [columns, setColumns] = useState<OptionalColumn[]>(() => readColumns(person.id));
  const [recentColumns, setRecentColumns] = useState<OptionalColumn[]>(() => readColumns(person.id, 'recent'));
  const [draft, setDraft] = useState({ source: q, text: q });
  if (draft.source !== q) setDraft({ source: q, text: q });
  const typed = draft.source === q ? draft.text : q;
  const choiceKey = `${scope}:${parent ?? 'all'}`;
  const [choice, setChoice] = useState(() => initialView(person.id, choiceKey, explicit));
  const source = explicit === undefined ? null : JSON.stringify(explicit);
  const [appliedSource, setAppliedSource] = useState(source);
  if (appliedSource !== source) {
    setAppliedSource(source);
    const next = explicit === undefined ? initialView(person.id, choiceKey) : validView(explicit);
    if (next.owner !== null && next.owner.id === choice.owner?.id) next.owner = choice.owner;
    if (JSON.stringify(next) !== JSON.stringify(choice)) setChoice(next);
  }
  const { kinds, owner, adminAccess, createdFrom, createdTo, activeFrom, sort, direction, arrangement, order: recentOrder } = choice;
  // The initial address is remembered too, as a link's explicit choices become this page's last view.
  const encoded = JSON.stringify(choice);
  useEffect(() => { if (source !== null) writeViewChoice(person.id, 'everything', 'view', encoded, choiceKey); }, [person.id, encoded, choiceKey, source]);
  const choose = (patch: Partial<EverythingView>) => {
    const next = { ...choice, ...patch };
    setChoice(next); writeViewChoice(person.id, 'everything', 'view', JSON.stringify(next), choiceKey); onView?.(next);
  };
  const setKinds = (next: readonly EverythingKind[] | ((before: readonly EverythingKind[]) => readonly EverythingKind[])) => {
    const selected = typeof next === 'function' ? next(kinds) : next;
    choose({ kinds: selected, ...(selected.some((kind) => !['org', 'project', 'topic', 'thread', 'document', 'chat'].includes(kind)) ? { navigation: false } : {}) });
  };
  const setOwner = (owner: OwnerChoice | null) => choose({ owner });
  const setCreatedFrom = (createdFrom: string) => choose({ createdFrom });
  const setCreatedTo = (createdTo: string) => choose({ createdTo });
  const setActiveFrom = (activeFrom: string) => choose({ activeFrom });
  const setAdminAccess = (adminAccess: boolean) => choose({ adminAccess });
  const setSort = (sort: EverythingFilter['sort']) => choose({ sort });
  const setDirection = (direction: EverythingFilter['direction']) => choose({ direction });
  const [filtersOpen, setFiltersOpen] = useState(false);
  const mode = useAdminMode();
  const participating = focus ?? choice.participating;
  const setParticipating = (value: boolean) => { choose({ participating: value }); onParticipation?.(value); };
  const chosenColumns = arrangement === 'recent' ? recentColumns : columns;
  const availableColumns = OPTIONAL_COLUMNS.filter((column) => (column !== 'admin_access' || mode.data?.as_admin === true)
    && (arrangement !== 'recent' || column !== 'state' && (recentOrder !== 'active' || column !== 'last_activity')));
  const shownColumns = chosenColumns.filter((column) => column !== 'admin_access' || mode.data?.as_admin === true);
  const [expansion, setExpansion] = useState<'all' | 'none' | 'manual'>('manual');
  const [expansionVersion, setExpansionVersion] = useState(0);
  const words = useDebounced(typed.trim());
  const filter: EverythingFilter = { scope, kinds, owner: owner?.id ?? null, createdFrom, createdTo, activeFrom, sort, direction, q: words,
    ...(arrangement === 'recent' ? { recent: recentOrder } : {}), navigation: choice.navigation, showArchived: choice.showArchived, hideDone: choice.hideDone,
    ...(participating ? { participating: true } : {}), ...(adminAccess ? { adminAccess: false } : {}) };
  const activeFilters = [kinds.length > 0, owner !== null, createdFrom !== '', createdTo !== '', activeFrom !== '', participating, adminAccess, !choice.showArchived, choice.hideDone].filter((on) => on).length;
  const [selecting, setSelecting] = useState(false);
  const selectionKey = JSON.stringify(filter);
  const [selection, setSelection] = useState<{ key: string; rows: ReadonlyMap<string, EverythingRow> }>({ key: selectionKey, rows: new Map() });
  const selected = selection.key === selectionKey ? selection.rows : new Map<string, EverythingRow>();
  const clearSelection = useCallback(() => setSelection((before) => before.key === selectionKey && before.rows.size === 0 ? before : { key: selectionKey, rows: new Map() }), [selectionKey, setSelection]);
  const viewKey = ['everything', person.id] as const;
  const sameView = (query: { queryKey: readonly unknown[] }) => JSON.stringify(query.queryKey[3]) === selectionKey;
  const refreshing = useIsFetching({ queryKey: viewKey, predicate: sameView }) > 0;
  const refresh = () => Promise.all(client.getQueryCache().findAll({ queryKey: viewKey, predicate: sameView }).filter((query) => query.isActive())
    .map((query) => client.refetchQueries({ queryKey: query.queryKey, exact: true })));
  const { locale } = useFormatContext();
  const toggleRow = (row: EverythingRow, on: boolean) => setSelection((before) => {
    const rows = new Map(before.key === selectionKey ? before.rows : []);
    const key = `${row.kind}:${row.id}`;
    if (on) rows.set(key, row); else rows.delete(key);
    return { key: selectionKey, rows };
  });
  const hasFilters = activeFilters > 0;
  const chooseColumn = (column: OptionalColumn, checked: boolean) => {
    const next = OPTIONAL_COLUMNS.filter((one) => one === column ? checked : chosenColumns.includes(one));
    if (arrangement === 'recent') setRecentColumns(next); else setColumns(next);
    saveColumns(person.id, next, arrangement);
  };
  const order = (column: Column) => {
    const next = COLUMN_SORT[column];
    choose({ sort: next, direction: sort === next && direction === 'asc' ? 'desc' : 'asc' });
  };
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
    <div className="flex flex-wrap items-center gap-3">
      <ArrangementSwitch wide value={arrangement} onChange={(arrangement) => choose({ arrangement, sort: arrangement === 'recent' ? recentOrder === 'opened' ? 'opened' : 'last_activity' : 'name', direction: arrangement === 'recent' ? 'desc' : 'asc' })} />
      {arrangement === 'recent' && <RecentOrderSelect value={recentOrder} onChange={(order) => choose({ order, sort: order === 'opened' ? 'opened' : 'last_activity', direction: 'desc' })} />}
    </div>
    <FilterBar wrap={false} search={<Input label={t('everything.search')} hideLabel type="search" placeholder={t('everything.searchPlaceholder')} autoComplete="off" value={typed}
        onChange={(event) => { setDraft({ source: q, text: event.target.value }); onQuery?.(event.target.value); }} fieldClassName="min-w-0" />}
      action={<Popover label={t('everything.filters')} open={filtersOpen} onOpenChange={setFiltersOpen} align="end"
        className="w-[min(var(--container-form),calc(100vw-2rem))] max-h-(--radix-popover-content-available-height) overflow-y-auto"
        trigger={<Button icon={<SlidersHorizontal aria-hidden />}>{hasFilters ? t('everything.activeFilters', { count: activeFilters }) : t('everything.filters')}</Button>}>
      {filtersOpen && <div className="flex flex-col gap-3">
      <ParticipationFilter checked={participating} onCheckedChange={setParticipating} />
      {person.is_system_admin && mode.data?.as_admin === true && onScope !== undefined && <Select label={t('everything.scope')} value={scope} onValueChange={onScope} options={[
        { value: 'mine', name: t('inventoryChoices.everything.scope.mine.name'), description: t('inventoryChoices.everything.scope.mine.description'), help: help(t('inventoryChoices.everything.scope.mine.name'), 'who-can-see-and-do-what') },
        { value: 'system', name: t('inventoryChoices.everything.scope.system.name'), description: t('inventoryChoices.everything.scope.system.description'), help: help(t('inventoryChoices.everything.scope.system.name'), 'who-can-see-and-do-what') },
      ]} />}
      {onScope === undefined && <p className="pb-2 text-sm text-text-muted">{t(`everything.scopes.${scope}`)}</p>}
      <div className="flex flex-col gap-1"><span className="text-sm text-text-muted">{t('everything.columns.kind')}</span>
        <Menu label={t('everything.kindsLabel')} trigger={<Button size="sm" variant="secondary">{kinds.length === 0 ? t('everything.allKinds') : t('everything.selectedKinds', { count: kinds.length })}</Button>}
          entries={KINDS.map((kind) => ({ type: 'check', id: kind, name: t(`inventoryChoices.everything.kinds.${kind}.name`), description: t(`inventoryChoices.everything.kinds.${kind}.description`),
            help: help(t(`inventoryChoices.everything.kinds.${kind}.name`), RESOURCE_GUIDES[kind] === undefined ? 'finding-things' : 'resources', RESOURCE_GUIDES[kind]),
            checked: kinds.includes(kind), onCheckedChange: (on) => setKinds((before) => on ? [...before, kind] : before.filter((one) => one !== kind)) }))} />
      </div>
      <OwnerFilter owner={owner} onChoose={setOwner} />
      <Checkbox {...choiceOption(t, 'navigationShow.archived')} name="show-archived" checked={choice.showArchived} onCheckedChange={(showArchived) => choose({ showArchived })} />
      <Checkbox {...choiceOption(t, 'navigationShow.hide_done')} name="hide-done" checked={choice.hideDone} onCheckedChange={(hideDone) => choose({ hideDone })} />
      <Checkbox name="hide-admin-only" label={t('inventoryChoices.everything.hideAdmin.name')} description={t('inventoryChoices.everything.hideAdmin.description')} help={help(t('inventoryChoices.everything.hideAdmin.name'), 'who-can-see-and-do-what')} checked={adminAccess} onCheckedChange={setAdminAccess} />
      <DateField label={t('everything.createdFrom')} value={createdFrom} max={createdTo || undefined} onValueChange={setCreatedFrom} />
      <DateField label={t('everything.activeFrom')} value={activeFrom} onValueChange={setActiveFrom} />
      <DateField label={t('everything.createdTo')} value={createdTo} min={createdFrom || undefined} onValueChange={setCreatedTo} />
      <Menu label={t('everything.sortLabel')} trigger={<Button size="sm" variant="secondary" icon={<ArrowDownUp aria-hidden />}>{t('everything.sortLabel')}</Button>}
        entries={[
          { type: 'radio', id: 'sort-column', label: t('everything.sortBy'), value: sort, onValueChange: (value) => { const column = SORT_COLUMNS.find((column) => COLUMN_SORT[column] === value); if (column !== undefined) setSort(COLUMN_SORT[column]); },
            options: SORT_COLUMNS.map((column) => ({ value: COLUMN_SORT[column], name: t(`inventoryChoices.everything.columns.${column}.name`), description: t(`inventoryChoices.everything.columns.${column}.description`) })) },
          { type: 'radio', id: 'sort-direction', label: t('everything.direction'), value: direction, onValueChange: (value) => { if (value === 'asc' || value === 'desc') setDirection(value); },
            options: (['asc', 'desc'] as const).map((value) => ({ value, name: t(`inventoryChoices.everything.directions.${value}.name`), description: t(`inventoryChoices.everything.directions.${value}.description`) })) },
        ]} />
      {hasFilters && <Button size="sm" variant="ghost" onClick={() => { choose({ kinds: [], owner: null, createdFrom: '', createdTo: '', activeFrom: '', participating: false, adminAccess: false, showArchived: true, hideDone: false, navigation: false }); onParticipation?.(false); }}>{t('everything.clearFilters')}</Button>}
      </div>}
    </Popover>} />
    <div role="group" aria-label={t('everything.sortLabel')} className="flex flex-wrap items-center justify-end gap-3">
      <p aria-live="polite" className="sr-only">{t(words === '' ? 'everything.sorted' : 'everything.sortedMatches', { column: t(`everything.columns.${sort}`), direction: t(`everything.directions.${direction}`) })}</p>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" variant="ghost" icon={<RefreshCw aria-hidden />} pending={refreshing} onClick={() => void refresh()}>{t(refreshing ? 'everything.refreshing' : 'everything.refresh')}</Button>
        <Button size="sm" variant="ghost" icon={<ListChecks aria-hidden />} aria-pressed={selecting} onClick={() => { setSelecting((before) => !before); setSelection({ key: selectionKey, rows: new Map() }); }}>{t('everything.select')}</Button>
        {arrangement === 'tree' && <><Button size="sm" variant="ghost" disabled={words !== ''} onClick={() => { setExpansion('all'); setExpansionVersion((before) => before + 1); }}>{t('everything.expandAll')}</Button>
        <Button size="sm" variant="ghost" onClick={() => { setExpansion('none'); setExpansionVersion((before) => before + 1); }}>{t('everything.collapseAll')}</Button></>}
        <Menu label={t('everything.chooseColumns')} trigger={<Button size="sm" icon={<Columns3 aria-hidden />}>{t('everything.chooseColumns')}</Button>}
          entries={availableColumns.map((column) => ({ type: 'check', id: column, name: t(`inventoryChoices.everything.columns.${column}.name`), description: t(`inventoryChoices.everything.columns.${column}.description`),
            ...(column === 'my_access' ? { help: help(t('inventoryChoices.everything.columns.my_access.name'), 'who-can-see-and-do-what', 'general-access') } : {}),
            checked: chosenColumns.includes(column), onCheckedChange: (on) => chooseColumn(column, on) }))} />
      </div>
    </div>
    {selecting && <div role="region" aria-label={t('everything.selection')} className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2">
      <span className="text-sm text-text-muted tabular-nums">{t('everything.selected', { number: formatNumber(selected.size, locale) })}</span>
      <Button size="sm" icon={<Share2 aria-hidden />} disabled={selected.size === 0} onClick={() => openAccessToMany([...selected.values()].flatMap((row) => row.can_share && row.kind !== 'group' && row.kind !== 'machine' && row.kind !== 'chat' ? [{ kind: row.kind, id: row.id, name: row.name ?? '' }] : []))}>{t('everything.share')}</Button>
      <Button size="sm" variant="ghost" disabled={selected.size === 0} onClick={() => setSelection({ key: selectionKey, rows: new Map() })}>{t('day100.batch.clear')}</Button>
    </div>}
    <ScrollRegion visibleScrollbars label={t('everything.tree')} kind="table" className="relative flex-none rounded-lg border border-border bg-surface sm:min-h-0 sm:flex-1">
      <EverythingTable key={`${scope}:${parent ?? ''}:${arrangement}:${expansionVersion}`} reader={person.id} filter={filter} extraColumns={arrangement === 'recent' ? shownColumns : []} columns={arrangement === 'recent' ? ['name', 'where', ...(recentOrder === 'opened' ? ['opened', 'last_activity'] as const : ['last_activity', 'activity_by'] as const), 'state', ...shownColumns.filter((column) => column !== 'last_activity' && column !== 'state')] : ['name', ...shownColumns]}
        selection={selecting ? { selected: new Set(selected.keys()), toggle: toggleRow, clear: clearSelection, all: (rows) => setSelection({ key: selectionKey, rows: new Map(rows.map((row) => [`${row.kind}:${row.id}`, row])) }) } : undefined}
        parent={parent} expansion={expansion} onSort={order} onOwner={(owner) => setOwner({ id: owner.id, name: owner.display_name })} />
    </ScrollRegion>
  </div>;
}

/** Also embedded by an org's settings; Management links to this same page in system scope. */
export function EverythingBrowser(props: EverythingBrowserProps) {
  const me = useQuery(meQuery);
  if (me.isPending) return <SkeletonRows rows={4} />;
  if (me.isError) return <ErrorState error={me.error} onRetry={() => void me.refetch()} />;
  return <Browser key={`${me.data.id}:${props.scope ?? 'mine'}:${props.parent ?? ''}:${props.q === undefined ? '' : 'controlled'}`} person={me.data} {...props} />;
}

/** The rail opens this in the main area, with no extra sidebar container. */
export function EverythingScreen(props: EverythingBrowserProps) {
  const { t } = useTranslation();
  useScreenHeading({ title: t('everything.title'), icon: ListTree });
  return <div className="flex min-h-full min-w-0 flex-col gap-4 p-(--page-padding) sm:h-full sm:min-h-0">
    <PageHeader title={t('everything.title')} />
    <EverythingBrowser {...props} />
  </div>;
}
