/** The environment's processes, this thread's own first, and signal controls.
 *
 * What runs in the environment as a tree of parents and children: its number, its command, the load it puts
 * on the processor and the memory it holds, how long it has run, and the thread and the tool call that
 * started it where that is known, linked to the step of the conversation. The filter "This thread" is on when the panel
 * opens. Whoever may manage shells sends a signal from the row's menu: Interrupt, Terminate or Kill, each asked
 * once and told what it does. The visible query shares the environment's process feed and closes
 * its subscription when no displayed view needs it.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { MoreHorizontal, SquareActivity } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { processesQuery, signalProcess } from '@/api/processes';
import type { WorkspaceAccess } from '@/features/files/door';
import type { ProcessInfo, ProcessSignal } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { formatSpan, useNow } from '@/lib/time';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Menu } from '@/ui/Menu';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';

import { ThreadViewFrame } from './parts';

interface Row {
  process: ProcessInfo;
  depth: number;
}

/** The processes laid out as a forest: a process under the one that started it when that one is in the list; this thread's own trees first. */
export function layOut(processes: readonly ProcessInfo[], threadId: string): Row[] {
  const byPid = new Map(processes.map((process) => [process.pid, process]));
  const children = new Map<number, ProcessInfo[]>();
  const roots: ProcessInfo[] = [];
  for (const process of processes) {
    if (byPid.has(process.ppid) && process.ppid !== process.pid) children.set(process.ppid, [...(children.get(process.ppid) ?? []), process]);
    else roots.push(process);
  }
  const owns = (process: ProcessInfo): boolean => process.thread_id === threadId || (children.get(process.pid) ?? []).some(owns);
  roots.sort((one, other) => Number(owns(other)) - Number(owns(one)) || one.pid - other.pid);
  const rows: Row[] = [];
  const visit = (process: ProcessInfo, depth: number) => {
    rows.push({ process, depth });
    for (const child of (children.get(process.pid) ?? []).sort((one, other) => one.pid - other.pid)) visit(child, depth + 1);
  };
  for (const root of roots) visit(root, 0);
  return rows;
}

const SIGNALS: { signal: ProcessSignal; key: 'interrupt' | 'terminate' | 'kill' }[] = [
  { signal: 'INT', key: 'interrupt' },
  { signal: 'TERM', key: 'terminate' },
  { signal: 'KILL', key: 'kill' },
];

function ProcessesPanel({ access }: { access: WorkspaceAccess }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const { locale } = useFormatContext();
  const now = useNow();
  const [mine, setMine] = useState(true);
  const [asked, setAsked] = useState<{ process: ProcessInfo; signal: ProcessSignal } | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const query = useQuery({ ...processesQuery(access.door), enabled: shown });

  const rows = useMemo(() => {
    const all = layOut(query.data?.processes ?? [], access.thread.id);
    return mine ? all.filter((row) => row.process.thread_id === access.thread.id) : all;
  }, [query.data, mine, access.thread.id]);

  if (query.error !== null && query.data === undefined) return <ErrorState compact className="h-full" error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data === undefined) return <SkeletonRows rows={5} />;

  const send = () => {
    if (asked === null) return;
    setPending(true);
    setError(null);
    signalProcess(access.door, asked.process.pid, asked.signal, asked.process.started_at).then(
      () => {
        setPending(false);
        setAsked(null);
      },
      (failure: unknown) => {
        setError(failure);
        setPending(false);
      },
    );
  };

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="processes-panel">
      <div className="border-b border-border px-3 py-2">
        <Switch label={t('processes.thisThread')} tooltip={t('processes.thisThreadAbout')} checked={mine} onCheckedChange={setMine} />
      </div>
      {rows.length === 0 ? (
        <EmptyState compact className="flex-1" icon={<SquareActivity aria-hidden />} title={mine ? t('processes.noneOwn') : t('processes.none')} description={mine ? t('processes.noneOwnAbout') : undefined} />
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          <table aria-label={t('processes.label')} className="w-full min-w-(--process-table-min) border-collapse text-left text-sm">
            <thead>
              <tr className="sticky top-0 z-(--layer-sticky) border-b border-border bg-bg-subtle text-xs font-medium text-text-muted">
                <th scope="col" className="h-8 px-3 font-medium">{t('processes.command')}</th>
                <th scope="col" className="px-2 text-right font-medium whitespace-nowrap">{t('processes.pid')}</th>
                <th scope="col" className="px-2 text-right font-medium whitespace-nowrap">{t('processes.cpu')}</th>
                <th scope="col" className="px-2 text-right font-medium whitespace-nowrap">{t('processes.memory')}</th>
                <th scope="col" className="w-8 px-1">
                  <span className="sr-only">{t('processes.actions')}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map(({ process, depth }) => {
                const own = process.thread_id === access.thread.id;
                // What started it and for how long it has run go under the command, so that the figures and the menu stay in view in a narrow bar.
                const origin =
                  process.thread_id === null ? (
                    <span>{t('processes.unknown')}</span>
                  ) : process.item_id === null ? (
                    <span>{own ? t('processes.ownThread') : t('processes.otherThread')}</span>
                  ) : (
                    <Link to="/thread/$threadId" params={{ threadId: process.thread_id }} search={{ item: process.item_id }} className="focus-inset rounded-md text-accent-text underline">
                      {own ? t('processes.ownStep') : t('processes.otherStep')}
                    </Link>
                  );
                return (
                  <tr key={`${process.pid}.${process.started_at}`} className={cx('border-b border-border last:border-b-0 hover:bg-bg-subtle', own && 'bg-accent-subtle/40')} data-testid="process-row">
                    <th scope="row" className="w-full max-w-0 min-w-0 px-3 py-1 text-left font-normal" style={{ paddingInlineStart: `calc(var(--spacing-3) + var(--spacing-4) * ${depth})` }}>
                      <span className="block truncate font-mono text-text" title={process.command}>{process.command}</span>
                      <span className="flex flex-wrap items-baseline gap-x-2 text-xs text-text-muted">
                        <span>{process.user}</span>
                        <span>
                          <span className="sr-only">{t('processes.age')}: </span>
                          {formatSpan(Math.max(0, now - Date.parse(process.started_at)), { locale })}
                        </span>
                        <span>
                          <span className="sr-only">{t('processes.startedBy')}: </span>
                          {origin}
                        </span>
                      </span>
                    </th>
                    <td className="px-2 text-right whitespace-nowrap tabular-nums">{process.pid}</td>
                    <td className="px-2 text-right whitespace-nowrap tabular-nums">{`${process.cpu_percent.toFixed(1)}%`}</td>
                    <td className="px-2 text-right whitespace-nowrap tabular-nums">{formatBytes(process.memory_bytes, locale)}</td>
                    <td className="px-1">
                      <Menu
                        label={t('processes.menuFor', { pid: process.pid })}
                        trigger={
                          <button type="button" aria-label={t('processes.menuFor', { pid: process.pid })} className="focus-inset inline-flex size-(--control-sm) items-center justify-center rounded-md text-text-muted hover:bg-border hover:text-text">
                            <MoreHorizontal aria-hidden />
                          </button>
                        }
                        entries={SIGNALS.map(({ signal, key }) => ({
                          id: signal,
                          label: t(`processes.signal.${key}`),
                          danger: signal === 'KILL',
                          disabled: access.mayShell ? false : t('processes.needsManage'),
                          onSelect: () => {
                            setError(null);
                            setAsked({ process, signal });
                          },
                        }))}
                      />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      {asked !== null && (
        <ConfirmDialog
          open
          onOpenChange={(next) => !next && setAsked(null)}
          title={t(`processes.confirm.${SIGNALS.find((one) => one.signal === asked.signal)?.key ?? 'interrupt'}`, { name: asked.process.name, pid: asked.process.pid })}
          confirmLabel={t(`processes.signal.${SIGNALS.find((one) => one.signal === asked.signal)?.key ?? 'interrupt'}`)}
          danger={asked.signal !== 'INT'}
          pending={pending}
          error={error}
          onConfirm={send}
        >
          {t(`processes.what.${SIGNALS.find((one) => one.signal === asked.signal)?.key ?? 'interrupt'}`)}
        </ConfirmDialog>
      )}
    </div>
  );
}

/** The Processes view of a thread. */
export function ProcessesView(_props: ViewProps) {
  return <ThreadViewFrame need="terminal.view" icon={<SquareActivity aria-hidden />}>{(access) => <ProcessesPanel access={access} />}</ThreadViewFrame>;
}
