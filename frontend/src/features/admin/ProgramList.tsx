/** A compact list of agent programs and their reported builds and status. */
import { useLayoutEffect, useRef, useState, type MouseEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { StateChip, type WorkState } from '@/ui/StateChip';
import { Table, type Column } from '@/ui/Table';

/** One build selected by the server for a platform. */
export interface ProgramListBuild {
  version: string;
  platform: string | null;
}

/** A named state supplied with the program's summary. */
export interface ProgramListStatus {
  state: WorkState;
  label: string;
}

/** The facts displayed in one administrator's program row. */
export interface ProgramListEntry {
  kind: string;
  name: string;
  protocol: string | null;
  shipped: readonly ProgramListBuild[];
  newestChecked: readonly ProgramListBuild[];
  channel: ProgramListStatus;
  check: ProgramListStatus;
}

// The shared table's widest column breakpoint leaves room for the six separate facts.
const SEPARATE_STATUS_MIN_PX = 1_000;
const SECONDARY_COLUMNS = { protocol: 'protocol', shipped: 'shipped', newestChecked: 'newestChecked' } as const;

/** Program rows keep their secondary facts when the containing pane narrows. */
export function ProgramList({ programs, onOpen }: { programs: readonly ProgramListEntry[]; onOpen: (kind: string) => void }) {
  const { t } = useTranslation();
  const frame = useRef<HTMLDivElement>(null);
  const [separateStatus, setSeparateStatus] = useState(false);
  useLayoutEffect(() => {
    const element = frame.current;
    if (element === null) return;
    const measure = () => setSeparateStatus(element.clientWidth >= SEPARATE_STATUS_MIN_PX);
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    measure();
    return () => observer.disconnect();
  }, []);
  const builds = (items: readonly ProgramListBuild[]) => items.length === 0 ? t('programList.notReported') : items.map((item) => item.platform === null ? item.version : `${item.version} · ${item.platform}`).join(t('startFlow.separator'));
  const protocol = (row: ProgramListEntry) => {
    const value = row.protocol?.trim();
    return value === undefined || value === '' ? t('programList.notReported') : value;
  };
  const detail = (label: string, value: string) => <span data-program-list-detail className="min-w-0 text-sm whitespace-normal wrap-anywhere text-text-muted">{t('programList.detail', { label, value })}</span>;
  const compact = (value: string) => <span tabIndex={0} data-overflow-text={value} className="focus-inset block max-w-(--field-narrow) truncate rounded-md" title={value}>{value}</span>;
  const open = (event: MouseEvent<HTMLAnchorElement>, kind: string) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    onOpen(kind);
  };
  const status = (value: ProgramListStatus) => <StateChip state={value.state} label={value.label} className="max-w-full" />;
  const columns: Column<ProgramListEntry>[] = [
    { id: 'name', header: t('programList.program'), cell: (row, shown) => <div className="flex min-w-0 flex-col gap-1">
      <a href={`/admin/harnesses?program=${encodeURIComponent(row.kind)}`} onClick={(event) => open(event, row.kind)} data-overflow-text={row.name} title={row.name} className="min-w-0 truncate font-medium text-accent-text hover:underline">{row.name}</a>
      <span tabIndex={0} data-overflow-text={row.kind} className="focus-inset min-w-0 truncate rounded-md text-sm text-text-muted" title={row.kind}>{row.kind}</span>
      {!shown.has(SECONDARY_COLUMNS.protocol) && detail(t('programList.protocol'), protocol(row))}
      {!shown.has(SECONDARY_COLUMNS.shipped) && detail(t('programList.shipped'), builds(row.shipped))}
      {!shown.has(SECONDARY_COLUMNS.newestChecked) && detail(t('programList.newestChecked'), builds(row.newestChecked))}
    </div> },
    { id: SECONDARY_COLUMNS.protocol, header: t('programList.protocol'), from: 'md', cell: (row) => compact(protocol(row)) },
    { id: SECONDARY_COLUMNS.shipped, header: t('programList.shipped'), from: 'md', cell: (row) => compact(builds(row.shipped)) },
    { id: SECONDARY_COLUMNS.newestChecked, header: t('programList.newestChecked'), from: 'lg', cell: (row) => compact(builds(row.newestChecked)) },
    ...(separateStatus ? [
      { id: 'channel', header: t('programList.channel'), cell: (row: ProgramListEntry) => status(row.channel) },
      { id: 'check', header: t('programList.check'), cell: (row: ProgramListEntry) => status(row.check) },
    ] : [
      { id: 'status', header: t('programList.status'), cell: (row: ProgramListEntry) => <div className="flex max-w-(--field-narrow) flex-wrap items-center gap-1">{status(row.channel)}{status(row.check)}</div> },
    ]),
  ];
  return <div ref={frame} className="min-w-0">
    {programs.length === 0 ? <EmptyState compact title={t('programList.empty')} /> : <Table boundedIdentity label={t('programList.title')} columns={columns} rows={programs} rowKey={(row) => row.kind} />}
  </div>;
}
