/** Common resource search, status and add controls.
 *
 * Every list of resources is laid out alike, so that they read as one product, from the outside to the
 * inside: the row of links to the kinds, which belongs to the section; then the kind, a mark and a name
 * with one line that says what it is for and the act that adds one; then the common filter bar;
 * then the rows, each with selection and its visible main act. What is searched for is written to the
 * address a moment after the typing stops, and the shared query returns the matching page. While the list is
 * loading it is drawn as rows, when it could not be loaded it says so with a way to try again, and when
 * it is empty it says why and offers the one act that fills it.
 */
import { useListChoice } from '@/lib/listState';
import { useViewedOrgState } from './HomePicker';
import { LibraryCounts } from './Library';
import { Hint } from '@/features/help/Hint';
import { useResourceCleanup } from '@/features/storage/acts';
import { holds } from '@/lib/permissions';
import { entryOf } from '@/features/resources/acts';
import type { LucideIcon } from 'lucide-react';
import { Search } from 'lucide-react';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { createContext, type ReactNode, useContext, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { meQuery } from '@/api/auth';
import { everythingQuery, type EverythingFilter, type ResourceFilter } from '@/api/everything';
import type { EverythingResource, EverythingScope, EverythingSort, ProviderListScope, ResourceFacts } from '@/generated/api-types';
import { INVENTORY_KINDS } from '@/features/resources/inventory';

import { AllBox, BulkBar, type ListActs, RowBox, RowMenuCell, RowPrimaryCell, useSelection, adminRow, secondaryEntries } from '@/features/resources/acts';
import { ResourceNav } from '@/features/resources/ResourceNav';
import { ResourceListControls } from '@/features/resources/ListControls';
import { ResourceOwner, type ResourceOwnership } from '@/features/resources/ResourceOwner';
import { type ResourceKind, rememberList, useResourceHeading } from '@/features/resources/trail';
import { useDebounced } from '@/lib/debounce';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { EmptyState } from '@/ui/EmptyState';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { adminSpec } from '@/ui/AdminAct';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { FilterBar } from '@/ui/FilterBar';
import { Button } from '@/ui/Button';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';

/** What a list needs to know of the request that loads it. */
export interface ListLoad {
  isPending: boolean;
  isError: boolean;
  error: unknown;
  isRefetching: boolean;
  refetch: () => unknown;
}

export interface ResourceRow extends ResourceOwnership { id: string; access?: Parameters<typeof holds>[0] }

const ListScope = createContext<{ scope: EverythingScope; embedded: boolean }>({ scope: 'mine', embedded: false });

/** A Management section reuses the real list, while its own frame owns the title and navigation. */
export function ResourceListScope({ scope, embedded = false, children }: { scope: EverythingScope; embedded?: boolean; children: ReactNode }) {
  return <ListScope value={{ scope, embedded }}>{children}</ListScope>;
}

export interface ResourceListFrameProps<Row extends ResourceRow> {
  /** Which kind of resource this is the list of: it says where the list lies, and what the way back from a resource of it leads to. */
  kind: ResourceKind;
  /** What the address says (the words typed, the filter chosen), kept so that the way back from one of its rows returns to the list as it was. */
  search: Readonly<Record<string, string | undefined>>;
  title: string;
  icon: LucideIcon;
  /** One line that says what this kind of resource is for. */
  lead: ReactNode;
  /** What the table is of, for a screen reader. */
  label: string;
  load: ListLoad;
  /** The loaded page rows. The frame never asks for the rest of an inventory. */
  all: readonly Row[];
  /** The rows the search and the filter leave. */
  rows: readonly Row[];
  columns: readonly Column<Row>[];
  rowKey: (row: Row) => string;
  rowClassName?: (row: Row) => string | undefined;
  /** The words typed in the search box, from `useSearchText`; the pager debounces its request. */
  text: string;
  onText: (text: string) => void;
  searchLabel: string;
  searchPlaceholder: string;
  /** The nested resource view, before search. */
  viewControl?: ReactNode;
  library?: ReadonlyMap<string, { facts: ResourceFacts; threads?: number | null }>;
  widen?: () => void;
  emptyView?: string;
  widerView?: string;
  /** The filters of the kind, drawn beside the search. */
  filters?: ReactNode;
  /** Order controls share their own compact popover instead of taking a row beside the criteria. */
  sorting?: ReactNode;
  hasSorting?: boolean;
  /** The one creation act, in the page header. */
  action?: ReactNode;
  /** What stands in place of the table when there are no rows at all. */
  empty: { icon: ReactNode; title: string; description?: string; action?: ReactNode };
  /** What stands in place of the table when the search and the filter leave nothing. */
  noMatch: string;
  /** Something between the toolbar and the table: a note about the list. */
  notice?: ReactNode;
  /** Something beside the title: an act that belongs to the kind as a whole. */
  headerActions?: ReactNode;
  /** What each row can do: the button and the menu at its end, the same menu under the right mouse button, and the acts rows that are ticked share. */
  acts?: ListActs<Row>;
  /** The pager resolves focus. This frame only renders the served rows. */
  participating?: boolean;
  onParticipation?: (value: boolean) => void;
  hasFilters?: boolean;
  /** A visible marker can include scope selection without changing the kind's empty-state wording. */
  filtersActive?: boolean;
  pagination?: ReactNode;
}

export interface ResourceListProps<Row extends ResourceRow> extends Omit<ResourceListFrameProps<Row>, 'load' | 'all' | 'rows' | 'columns' | 'participating' | 'onParticipation' | 'hasFilters' | 'filtersActive' | 'sorting' | 'hasSorting' | 'pagination'> {
  /** The existing kind-specific DTO, selected from this page's tagged resource view. */
  readRow: (resource: EverythingResource) => Row | null;
  filter?: ResourceFilter;
  columns: readonly Column<Row>[] | ((rows: readonly Row[]) => readonly Column<Row>[]);
  poll?: { interval: number; while: (row: Row) => boolean };
}

/** Every resource kind has the same query, continuation, focus and name filter. */
export function ResourceList<Row extends ResourceRow>({ readRow, filter: resource, columns, poll: _poll, ...props }: ResourceListProps<Row>) {
  const { t } = useTranslation();
  const { scope } = useContext(ListScope);
  const me = useQuery(meQuery);
  const sorting = ['name', 'owner', 'created', 'last_activity', ...(props.kind === 'mounts' || props.kind === 'machines' ? [] : ['use'] as const), ...(props.kind === 'hosts' ? ['load', 'running'] as const : [])] as const;
  const [participating, setParticipating] = useListChoice(`resources.${props.kind}`, 'participating', false);
  const [keptSort, setSort] = useListChoice<EverythingSort>(`resources.${props.kind}`, 'sort', 'name');
  const [keptDirection, setDirection] = useListChoice<EverythingFilter['direction']>(`resources.${props.kind}`, 'direction', 'asc');
  const sort=sorting.some((value)=>value===keptSort)?keptSort:'name';
  const direction=keptDirection==='desc'?'desc':'asc';
  const {org:viewed,load:orgLoad} = useViewedOrgState();
  const defaultView: ProviderListScope = props.kind === 'environments' ? 'viewable' : 'usable';
  const [keptView, chooseView] = useListChoice<ProviderListScope>(`resources.${props.kind}`, 'view', resource?.view ?? defaultView);
  const offered: ProviderListScope[] = ['mine', 'managed', ...(props.kind === 'environments' ? [] : ['usable' as const]), 'viewable', ...(viewed?.access.administers === true ? ['all' as const] : [])];
  const view = offered.includes(keptView) ? keptView : defaultView;
  useEffect(()=>{
    if(keptView!==view&&(keptView!=='all'||orgLoad.isSuccess))chooseView(view);
    if(keptSort!==sort)setSort(sort);
    if(keptDirection!==direction)setDirection(direction);
  },[keptView,view,orgLoad.isSuccess,chooseView,keptSort,sort,setSort,keptDirection,direction,setDirection]);
  const wider = offered.slice(offered.indexOf(view)+1).find((value):value is Exclude<ProviderListScope,'mine'>=>value!=='mine');
  const text = useDebounced(props.text.trim());
  const filter: EverythingFilter = { scope, kinds: [INVENTORY_KINDS[props.kind]], q: text,
    owner: null, createdFrom: '', createdTo: '', sort, direction, participating,
    resource: { ...resource, view, ...(view === 'all' && viewed !== null ? {home:viewed.id} : {}), details: true } };
  const options = everythingQuery(me.data?.id ?? '', 'resources', filter, props.kind === 'mcp' ? 'mcp-servers' : props.kind);
  const load = useInfiniteQuery({ ...options, enabled: me.data !== undefined && (keptView!=='all'||orgLoad.isSuccess) });
  const rows = (load.data?.pages.flatMap((page) => page.items) ?? []).flatMap((item) => {
    if (item.resource === undefined || item.resource === null) return [];
    const row = readRow(item.resource);
    return row === null ? [] : [row];
  });
  const hasFilters = props.text.trim() !== '' || participating || resource?.state !== undefined || resource?.kind !== undefined || resource?.harness !== undefined;
  const filtersActive = hasFilters || view !== defaultView;
  const hasSorting = sort !== 'name' || direction !== 'asc';
  const library = new Map((load.data?.pages.flatMap((page) => page.items) ?? []).flatMap((row) => row.library === null || row.library === undefined ? [] : [[row.id, {facts:row.library,threads:row.library_threads}] as const]));
  return <ResourceListFrame {...props} library={library}
    emptyView={t(`library.empty.${view}`)} widerView={wider === undefined ? undefined : t(`library.widen.${wider}`)} widen={wider === undefined ? undefined : () => chooseView(wider)}
    viewControl={<Select size="sm" hideLabel label={t('library.show')} value={view} onValueChange={chooseView}
      fieldClassName="min-w-0 max-w-[45%] shrink-0" className="max-w-full"
      options={offered.map((value) => ({value,name:value==='all'?t('library.views.all',{org:viewed?.name}):t(`library.views.${value}`),separatorBefore:value==='all'}))} />}
    columns={typeof columns === 'function' ? columns(rows) : columns}
    searchPlaceholder={t('resources.inventorySearch')}
    all={rows} rows={rows} load={me.isError ? me : keptView==='all'&&orgLoad.isError?orgLoad:load} participating={participating} onParticipation={setParticipating} hasFilters={hasFilters}
    filtersActive={filtersActive}
    filters={props.filters} hasSorting={hasSorting}
    sorting={<><Select size="sm" label={t('everything.sortBy')} value={sort} onValueChange={(value) => { if (sorting.some((one) => one === value)) { setSort(value); if (value === 'use' && sort !== 'use') setDirection('desc'); } }}
      options={sorting.map((value) => ({ value, name: t(`inventoryChoices.everything.columns.${value}.name`), description: t(`inventoryChoices.everything.columns.${value}.description`) }))} />
      <Select size="sm" label={t('everything.direction')} value={direction} onValueChange={setDirection} options={(['asc', 'desc'] as const).map((value) => ({ value,
        name: t(`inventoryChoices.everything.directions.${value}.name`), description: t(`inventoryChoices.everything.directions.${value}.description`) }))} /></>}
    pagination={<>{load.isFetchNextPageError && <ErrorState compact error={load.error} retrying={load.isFetchingNextPage} onRetry={() => void load.fetchNextPage()} />}
      {load.hasNextPage && <Button pending={load.isFetchingNextPage} onClick={() => void load.fetchNextPage()}>{t('everything.more')}</Button>}</>} />;
}

export function ResourceListFrame<Row extends ResourceRow>({
  kind,
  search,
  title,
  icon: Icon,
  lead,
  label,
  load,
  all,
  rows: matches,
  columns: suppliedColumns,
  rowKey,
  rowClassName,
  text,
  onText,
  searchLabel,
  searchPlaceholder,
  filters,
  viewControl,
  library,
  widen,
  emptyView,
  widerView,
  sorting,
  hasSorting = false,
  action,
  empty,
  noMatch,
  notice,
  headerActions,
  acts,
  participating: suppliedParticipation,
  onParticipation,
  hasFilters = false,
  filtersActive,
  pagination,
}: ResourceListFrameProps<Row>) {
  const { t } = useTranslation();
  const { embedded } = useContext(ListScope);
  // One owner renderer and one narrow fallback serve every resource kind, including machines and profiles.
  const emptyAction = action ? undefined : empty.action;
  const hasHeaderActions = Boolean(action) || Boolean(headerActions);
  const baseColumns: readonly Column<Row>[] = suppliedColumns.some((column) => column.id === 'owner') ? suppliedColumns : [
    ...suppliedColumns.slice(0, 1), { id: 'owner', header: t('resources.listOwner'), from: 'md' as const, cell: () => null }, ...suppliedColumns.slice(1),
  ];
  const columns = baseColumns.map<Column<Row>>((column) => column.id === 'owner' ? { ...column, from: column.from ?? 'md', cell: (row: Row) => <ResourceOwner {...row} /> }
    : column.id !== 'name' ? column : { ...column, cell: (row: Row, shown: ReadonlySet<string>) => {
      const showOwner = !shown.has('owner');
      const entry=library?.get(row.id);
      return <div className="flex min-w-0 flex-col gap-1">
      <div className="flex min-w-0 flex-wrap items-start gap-x-2 gap-y-1"><div data-resource-name className="min-w-0 flex-1 basis-[min(100%,12rem)]">{column.cell(row, shown)}</div><AsAdminMark of={adminAccessOf(row)?.place} responsive /></div>
      {showOwner && <ResourceOwner {...row} />}
      {entry !== undefined && <LibraryCounts facts={entry.facts} threads={entry.threads} />}
    </div>; } });
  const participating = suppliedParticipation ?? false;
  const setParticipating = (value: boolean) => onParticipation?.(value);
  const rows = matches;
  const permission = kind === 'machines' ? ['machine.manage', 'host.manage'] : `${kind === 'mcp' ? 'mcp' : ({ providers: 'provider', agents: 'agent', environments: 'environment', templates: 'template', hosts: 'host', mounts: 'mount', skills: 'skill' } as const)[kind]}.manage`;
  const cleanup = useResourceCleanup();
  const objectKind = ({providers:'provider',agents:'agent',environments:'environment',templates:'template',hosts:'host',mounts:'mount',mcp:'mcp_server',skills:'skill',machines:'machine'} as const)[kind];
  const retainsStorage = kind !== 'agents' && kind !== 'mcp';
  const actsOf = (row: Row) => {
    if (acts === undefined) return undefined;
    const current = acts.of(row);
    const entries = !retainsStorage || current.entries.some((entry) => entry.type === undefined && entry.id === 'cleanup') ? current.entries : [...current.entries, {type:'separator' as const,id:'before-cleanup'}, entryOf(cleanup.act({kind:objectKind,id:row.id},holds(row.access,Array.isArray(permission) ? 'machine.manage' : permission)))];
    return adminRow({...current,entries},adminSpec(row,permission));
  };
  useResourceHeading(kind, Icon, undefined, undefined, !embedded);
  const kept = JSON.stringify(search);
  useEffect(() => { if (!embedded) rememberList(kind, JSON.parse(kept) as Record<string, string>); }, [kind, kept, embedded]);
  const selection = useSelection(rows, rowKey);
  const bulk = acts?.bulk ?? [];
  // Selection is part of every resource list. A read-only selection can still be counted and cleared;
  // only acts all selected rows permit are offered by BulkBar.
  const ticking = acts !== undefined;

  // The columns the kind draws, with a box to tick a row before them where rows share acts, and the row's own acts after them.
  const shown: readonly Column<Row>[] =
    acts === undefined
      ? columns
      : [
          ...(!ticking
            ? []
            : [
                {
                  id: 'select',
                  header: <AllBox checked={selection.all} onChange={selection.toggleAll} />,
                  className: 'w-px',
                  cell: (row: Row) => <RowBox name={acts.name(row)} checked={selection.isTicked(row)} onChange={(on) => selection.toggle(row, on)} />,
                },
              ]),
          ...columns,
          // The main act stays visible on touch screens too; secondary facts yield room first.
          {
            id: 'primary',
            header: t('resources.act.primaryColumn'),
            hideHeader: true,
            align: 'end' as const,
            className: 'w-px pr-0 whitespace-nowrap',
            cell: (row: Row) => <RowPrimaryCell name={acts.name(row)} acts={actsOf(row) ?? acts.of(row)} />,
          },
          {
            id: 'acts',
            header: t('resources.act.column'),
            hideHeader: true,
            align: 'end' as const,
            className: 'w-px whitespace-nowrap',
            cell: (row: Row, columnsShown: ReadonlySet<string>) => <RowMenuCell name={acts.menuName?.(row) ?? acts.name(row)} acts={actsOf(row) ?? acts.of(row)} working={!columnsShown.has('primary')} />,
          },
        ];

  const body = (
    <>
      {!embedded && <ResourceNav />}
      {!embedded && <div data-resource-kind={kind}><PageHeader
        mark={
          <PlaceMark>
            <Icon aria-hidden />
          </PlaceMark>
        }
        title={title}
        lead={lead}
        actions={hasHeaderActions ? <>{action}{headerActions}</> : undefined}
      /></div>}
      {embedded && hasHeaderActions && <div className="flex flex-wrap items-center justify-end gap-2">{action}{headerActions}</div>}
      <FilterBar wrap={false} leading={viewControl===undefined?undefined:<>{viewControl}<Hint id="resource-library" popover /></>}
        search={<SearchBox text={text} onText={onText} label={searchLabel} placeholder={searchPlaceholder} />}>
        <ResourceListControls filters={filters} sorting={sorting} participating={participating} onParticipation={setParticipating} filtered={filtersActive ?? (hasFilters || participating)} sorted={hasSorting} />
      </FilterBar>
      {ticking && selection.selected.length > 0 && <BulkBar rows={selection.selected} acts={bulk} onClear={selection.clear} permission={permission} />}
      {notice}
      {load.isPending ? (
        <SkeletonRows rows={4} />
      ) : load.isError && all.length === 0 ? (
        <ErrorState compact error={load.error} retrying={load.isRefetching} onRetry={() => void load.refetch()} />
      ) : all.length === 0 && !hasFilters && emptyView !== undefined ? (
        <EmptyState compact icon={empty.icon} title={emptyView} action={widen === undefined ? undefined : <Button onClick={widen}>{widerView}</Button>} />
      ) : all.length === 0 && !hasFilters ? (
        <EmptyState icon={empty.icon} title={empty.title} {...(empty.description === undefined ? {} : { description: empty.description })} {...(emptyAction === undefined ? {} : { action: emptyAction })} />
      ) : rows.length === 0 ? (
        <EmptyState compact icon={<Search aria-hidden />} title={noMatch} />
      ) : (
        <Table
          boundedIdentity
          label={label}
          columns={shown}
          rows={rows}
          rowKey={rowKey}
          {...(rowClassName === undefined ? {} : { rowClassName })}
          {...(acts === undefined ? {} : { rowMenu: { label: (row: Row) => t('resources.act.menuOf', { name: acts.menuName?.(row) ?? acts.name(row) }), entries: (row: Row) => secondaryEntries(actsOf(row) ?? acts.of(row)), keys: (row: Row) => acts.of(row).keys } })}
        />
      )}
      {pagination}
      {acts?.dialogs}
      {cleanup.dialogs}
    </>
  );
  return embedded ? <div className="flex min-w-0 flex-col gap-4">{body}</div> : <Page>{body}</Page>;
}

/**
 * The words typed in the search box of a list. The field follows typing at once; the address is
 * written once the typing has rested, and a change of the address from outside (the back button, a
 * link) is taken into the box.
 */
export function useSearchText(q: string, write: (q: string) => void): [string, (text: string) => void] {
  const [text, setText] = useState(q);
  const settled = useDebounced(text);
  const written = useRef(q);
  useEffect(() => {
    if (settled === written.current) return;
    written.current = settled;
    write(settled);
  }, [settled, write]);
  useEffect(() => {
    if (q === written.current) return;
    written.current = q;
    setText(q);
  }, [q]);
  return [text, setText];
}

function SearchBox({ text, onText, label, placeholder }: { text: string; onText: (text: string) => void; label: string; placeholder: string }) {
  return (
    <Input
      label={label}
      hideLabel
      type="search"
      placeholder={placeholder}
      value={text}
      onChange={(event) => onText(event.target.value)}
      // Filter and Sort now have short triggers; search uses their remaining row width.
      fieldClassName="min-w-0 w-full"
      autoComplete="off"
    />
  );
}
