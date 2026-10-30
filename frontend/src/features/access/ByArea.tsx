/** The full form: one cell for each row, following its place or cut loose, locks, the advanced rows, and Administration.
 *
 * The same page as the short form with every row in view: the areas down the side, View, the middle
 * level and Manage across, and under them the rows of one act each and the rows about running the place.
 * A cell says whom its row names and, quietly, whom it follows from the place above; a cell that a lock
 * above decides shows the lock and has no menu. The page's history is under it.
 */
import { ArrowLeft, History } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { useInfiniteQuery } from '@tanstack/react-query';

import { type AccessTarget, type AccessRowTarget, historyQuery } from '@/api/access';
import type { NodeOrSystem, PermissionPage, RowProfile } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { HistoryLines } from '@/ui/HistoryLines';
import { SkeletonRows } from '@/ui/Skeleton';
import { Disclosure } from '@/ui/Disclosure';

export { Disclosure } from '@/ui/Disclosure';

import type { Asked } from './ChangePreview';
import type { Changes, PageSource } from './changes';
import { containerOf, followedPlace, type Catalogue, type Cell, type Grid, type GridArea, gridOf, middleOf, rowOnPage } from './model';
import { permissionNameOf } from './names';
import { outsiderReason } from './Outsiders';
import { ChangeFailed, StaleNotice } from './ShortForm';
import { RowCell } from './RowCell';

/** The four doors of an environment, whose Manage cell says what it gives before it is given. */
const DOORS: ReadonlySet<string> = new Set(['files', 'terminal', 'browser', 'vnc', 'ports']);

type Column = 'view' | 'middle' | 'manage';

/** The columns of an area, each with its cell: an area with no middle level (an environment, its doors) has two. */
function columnsOf(area: GridArea, middle: boolean): readonly (readonly [Column, Cell | null])[] {
  return [
    ['view', area.view],
    ...(middle ? ([['middle', area.middle]] as const) : []),
    ['manage', area.manage],
  ];
}

const ROW_BASE = 'grid grid-cols-1 gap-x-4 gap-y-1 border-b border-border py-3 last:border-b-0';
const THREE = 'sm:grid-cols-[7rem_repeat(3,minmax(0,1fr))]';
const TWO = 'sm:grid-cols-[7rem_repeat(2,minmax(0,1fr))]';

interface GridViewProps {
  grid: Grid;
  target: AccessRowTarget;
  source: PageSource;
  changes: Changes;
  ask: (asked: Asked) => void;
  profile: RowProfile;
  page: PermissionPage;
  /** What the thing's own area is called, which is what the column of the middle level is named for. */
  kind: string;
  onOpenLock?: ((place: NodeOrSystem) => void) | undefined;
  /** Rows the grid leaves out of Advanced: the caller shows them elsewhere. */
  hideAdvanced?: boolean;
}

function Label({ children }: { children: ReactNode }) {
  return <span className="text-xs font-medium text-text-muted sm:hidden">{children}</span>;
}

/** The areas of a page as a grid of cells, then the rows of one act each and the rows about running the place. */
export function GridView({ grid, target, source, changes, ask, profile, page, kind, onOpenLock, hideAdvanced = false }: GridViewProps) {
  const { t } = useTranslation();
  const container = containerOf(page) ?? followedPlace(page.rows);
  const middle = middleOf(kind);
  // Some areas have no middle level: an environment and its doors are looked at, or managed.
  const hasMiddle = grid.areas.some((area) => area.middle !== null);
  const shape = hasMiddle ? THREE : TWO;
  const outsiderOff = outsiderReason(t, page, target);
  const cellFor = (cell: Cell, extra?: ReactNode) => (
    <RowCell
      cell={cell}
      target={target}
      source={source}
      changes={changes}
      ask={ask}
      place={container}
      profile={profile}
      onOpenLock={onOpenLock}
      outsiderOff={outsiderOff}
      aside={extra}
    />
  );
  const areaName = (area: string) => area === 'ports' ? t('ports.title') : t(`access.area.${area}`, { defaultValue: area.charAt(0).toUpperCase() + area.slice(1) });
  const levelName = (cell: Cell) => (cell.def.level === 'edit' && middle !== 'edit' ? t('access.level.edit') : cell.def.level === 'use' && middle !== 'use' ? t('access.level.use') : null);

  return (
    <div className="flex flex-col gap-4">
      <div role="table" aria-label={t('access.grid.label')} className="rounded-lg border border-border px-3">
        <div role="row" className={cx('hidden gap-x-4 border-b border-border py-2 text-xs font-medium text-text-muted sm:grid', hasMiddle ? 'grid-cols-[7rem_repeat(3,minmax(0,1fr))]' : 'grid-cols-[7rem_repeat(2,minmax(0,1fr))]')}>
          <span role="columnheader">{t('access.grid.area')}</span>
          <span role="columnheader">{t('access.grid.view')}</span>
          {hasMiddle && <span role="columnheader">{t(`access.grid.${middle}`)}</span>}
          <span role="columnheader">{t('access.grid.manage')}</span>
        </div>
        {grid.areas.map((area) => {
          return (
            <div key={area.area} role="row" className={cx(ROW_BASE, shape)}>
              <span role="rowheader" className="text-sm font-medium text-text">
                {areaName(area.area)}
              </span>
              {columnsOf(area, hasMiddle).map(([column, cell]) => (
                <div key={column} role="cell" className="flex min-w-0 flex-col gap-1">
                  <Label>{t(column === 'view' ? 'access.grid.view' : column === 'manage' ? 'access.grid.manage' : `access.grid.${middle}`)}</Label>
                  {cell === null ? (
                    <span className="text-sm text-text-muted" aria-label={t('access.grid.none')}>
                      —
                    </span>
                  ) : (
                    <>
                      {cellFor(cell)}
                      {levelName(cell) !== null && <span className="text-xs text-text-muted">({levelName(cell)})</span>}
                      {column === 'manage' && DOORS.has(area.area) && <span className="text-xs text-text-muted">{area.area === 'ports' ? t('ports.manageMeaning') : t(`access.doors.manage.${area.area as 'files' | 'terminal' | 'browser' | 'vnc'}`)}</span>}
                    </>
                  )}
                </div>
              ))}
            </div>
          );
        })}
      </div>

      {!hideAdvanced && grid.advanced.length > 0 && (
        <Disclosure title={t('access.advanced.title')}>
          {grid.advanced.map((cell) => (
            <div key={cell.def.name + cell.row.profile} className="grid grid-cols-1 gap-x-4 gap-y-1 py-2 sm:grid-cols-[10rem_minmax(0,1fr)]">
              <span className="text-sm text-text">{permissionNameOf(cell.def)}</span>
              {cellFor(cell)}
            </div>
          ))}
        </Disclosure>
      )}

      {grid.administration.length > 0 && (
        <Disclosure title={t('access.administration.title')} lead={t('access.administration.lead')}>
          {grid.administration.map((cell) => (
            <div key={cell.def.name + cell.row.profile} className="grid grid-cols-1 gap-x-4 gap-y-1 py-2 sm:grid-cols-[10rem_minmax(0,1fr)]">
              <span className="text-sm text-text">{permissionNameOf(cell.def)}</span>
              {cellFor(cell)}
            </div>
          ))}
        </Disclosure>
      )}
    </div>
  );
}

/** Every permission-page kind has history; read it only when its disclosure is opened. */
function PageHistory({ target }: { target: AccessTarget }) {
  const { t } = useTranslation();
  const history = useInfiniteQuery(historyQuery(target));
  const nextError: unknown = history.isFetchNextPageError ? history.error : undefined;
  if (history.isPending) return <SkeletonRows rows={3} />;
  if (history.isError) return <ErrorState compact error={history.error} title={t('access.history.notLoaded')} retrying={history.isRefetching} onRetry={() => void history.refetch()} />;
  const lines = history.data.pages.flatMap((entry) => entry.items);
  if (lines.length === 0) return <EmptyState compact title={t('access.history.none')} />;
  return (
    <HistoryLines
      lines={lines}
      hasMore={history.hasNextPage}
      loading={history.isFetchingNextPage}
      error={nextError}
      onMore={() => void history.fetchNextPage()}
    />
  );
}

interface ByAreaProps {
  target: AccessTarget;
  page: PermissionPage;
  catalogue: Catalogue;
  source: PageSource;
  changes: Changes;
  ask: (asked: Asked) => void;
  onBack: () => void;
}

export function ByArea({ target, page, catalogue, source, changes, ask, onBack }: ByAreaProps) {
  const { t } = useTranslation();
  const [showHistory, setShowHistory] = useState(false);
  const grid = gridOf(catalogue.of(target.kind), rowOnPage(page));
  return (
    <div className="flex flex-col gap-4">
      <StaleNotice changes={changes} />
      <ChangeFailed changes={changes} />
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-text-muted">{changes.busy && changes.doing !== null ? <span role="status">{changes.doing}…</span> : t('access.grid.lead')}</p>
        <Button size="sm" icon={<ArrowLeft aria-hidden />} onClick={onBack}>
          {t('access.grid.back')}
        </Button>
      </div>
      <GridView grid={grid} target={target} source={source} changes={changes} ask={ask} profile="any" page={page} kind={target.kind} />
      <div className={cx('rounded-lg border border-border')}>
        <button
          type="button"
          aria-expanded={showHistory}
          className="flex w-full cursor-default items-center gap-2 rounded-lg px-3 py-2 text-left text-sm font-medium text-text hover:bg-bg-subtle"
          onClick={() => setShowHistory((on) => !on)}
        >
          <History className="text-text-subtle" size={16} aria-hidden />
          {t('access.history.title')}
        </button>
        {showHistory && (
          <div className="border-t border-border px-3">
            <PageHistory target={target} />
          </div>
        )}
      </div>
    </div>
  );
}
