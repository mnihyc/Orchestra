/** The caller's own shells in the thread's environment, whichever screen each was opened from: taken up again with what scrolled past, one screen at a time, how long they stay and how many, the choice of which to close for one more, Add to message, and the note on what a shell there reaches.
 *
 * A strip of the person's shells, each shown by the directory it began in and by what it is doing: at its prompt,
 * open on another screen (and the way to take it here), or ended (and why). A shell is its person's own and stays
 * while they are away; when they come back the shell is where it was and shows its screen as it was, with what
 * scrolled past above it. At as many shells as is allowed, "+" asks which to give up instead of refusing. Opening
 * is refused with the reason in words where the environment, a policy or the machine's owner forbids it.
 */
import { useViewInstance } from '@/features/workbench/instance';
import { useMutation, useQuery } from '@tanstack/react-query';
import { Plus, SquareTerminal, TriangleAlert, X } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AtLimit, atLimit, closeShell, closedShellsQuery, notAllowedReason, openShell, shellsQuery } from '@/api/terminals';
import { useEnvironment, usePanelWorkspaceDoor, type EnvironmentWorkspaceAccess } from '@/features/files/door';
import type { TerminalSession } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { ViewActions } from '@/features/workbench/actions';
import { formatTimeOfDay, useNow } from '@/lib/time';
import { useFormatContext } from '@/lib/appearance';
import { type ShellConnection, type ShellState } from '@/realtime/terminal';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';
import { FoldingStrip } from '@/ui/FoldingStrip';
import { Tooltip } from '@/ui/Tooltip';
import { HelpHint } from '@/ui/HelpHint';

import { ThreadViewFrame } from './parts';
import { TerminalScreen } from './TerminalScreen';

/** The round trip above which a tab shows it: a key that appears late is then understood and not typed again. */
export const SLOW_ROUND_TRIP_MS = 150;
/** A shell closed longer ago than this is no longer offered: a day is long enough to find out what happened to it. */
const CLOSED_SHOWN_MS = 24 * 60 * 60 * 1000;
/** How long a shell is kept for a person who is away, until the place says otherwise: the shipped setting. */
const KEPT_HOURS_DEFAULT = 8;

const baseName = (path: string): string => (path === '' ? '' : (path.split('/').filter(Boolean).at(-1) ?? ''));

function LimitDialog({ limit, onClose, onChosen, pending, error, environmentName }: { limit: AtLimit; onClose: () => void; onChosen: (ids: string[]) => void; pending: boolean; error: unknown; environmentName: string }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  // The oldest is ticked: it is the one that has been left longest.
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set(limit.mine[0] === undefined ? [] : [limit.mine[0].id]));
  const own = limit.limit === 'person' && limit.mine.length > 0;
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !pending && onClose()}
      title={own ? t('terminal.limit.title') : t('terminal.limit.environmentTitle')}
      description={own ? t('terminal.limit.about', { count: limit.mine.length, environment: environmentName }) : t('terminal.limit.environmentAbout', { count: limit.people ?? 0, environment: environmentName })}
      size="sm"
      dismissible={!pending}
      footer={
        <>
          <Button onClick={onClose} disabled={pending}>
            {t('common.cancel')}
          </Button>
          {own && (
            <Button variant="primary" pending={pending} disabled={ticked.size === 0} onClick={() => onChosen([...ticked])}>
              {t('terminal.limit.confirm', { count: ticked.size })}
            </Button>
          )}
        </>
      }
    >
      {own && (
        <ul className="flex flex-col gap-2" aria-label={t('terminal.limit.list')}>
          {limit.mine.map((shell) => (
            <li key={shell.id}>
              <Checkbox
                label={`${shell.cwd === '' ? t('terminal.strip.root') : shell.cwd}: ${t('terminal.limit.lastUsed', { time: formatTimeOfDay(shell.last_io_at, format) })}${shell.busy ? `, ${t('terminal.limit.running')}` : ''}`}
                checked={ticked.has(shell.id)}
                onCheckedChange={(on) => setTicked((before) => new Set(on ? [...before, shell.id] : [...before].filter((one) => one !== shell.id)))}
              />
            </li>
          ))}
        </ul>
      )}
      {error !== null && <ErrorAlert error={error} className="mt-3" />}
    </Dialog>
  );
}

export function TerminalPanel({ access, tab }: { access: EnvironmentWorkspaceAccess; tab?: ViewProps['tab'] }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { door } = access;
  const { environment } = useEnvironment(door.environmentId);
  const shells = useQuery(shellsQuery(door));
  const closed = useQuery(closedShellsQuery(door));
  const [selected, setSelected] = useState<string | null>(null);
  /** The ticket of a shell that was just opened: its first attachment uses it. */
  const [first, setFirst] = useState<Record<string, string>>({});
  /** Shells this window has taken: the others that are attached are on another screen. */
  const [here, setHere] = useState<ReadonlySet<string>>(new Set());
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  /** The connection this window holds, and the shell it is of. */
  const [held, setHeld] = useState<{ id: string; connection: ShellConnection } | null>(null);
  const [live, setLive] = useState<ShellState | null>(null);
  const [limit, setLimit] = useState<AtLimit | null>(null);

  useEffect(() => {
    if (held === null) return;
    const { id, connection } = held;
    const watch = () => {
      const state = connection.state.getState();
      setLive(state);
      // A shell this window has attached is this window's: the list says it is attached a moment later, and that is this window.
      if (state.phase === 'attached') setHere((before) => (before.has(id) ? before : new Set([...before, id])));
    };
    watch();
    return connection.state.subscribe(watch);
  }, [held]);

  const open = useMutation({
    mutationFn: (close: string[]) => openShell(door, { cols: 80, rows: 24, cwd: access.directory, ...(close.length === 0 ? {} : { close }) }),
    onSuccess: (reply) => {
      setLimit(null);
      setFirst((before) => ({ ...before, [reply.session.id]: reply.ticket }));
      setHere((before) => new Set([...before, reply.session.id]));
      setSelected(reply.session.id);
    },
    onError: (error) => {
      const full = atLimit(error);
      if (full !== null) setLimit(full);
    },
  });

  const close = useMutation({
    mutationFn: (id: string) => closeShell(id),
    onSuccess: (_session, id) => {
      setHere((before) => new Set([...before].filter((one) => one !== id)));
    },
  });

  const now = useNow();
  const sessions = useMemo<TerminalSession[]>(() => {
    const open = shells.data ?? [];
    // A shell its person closed themselves is no news to them: what is shown is what ended without them.
    const ended = (closed.data ?? []).filter((shell) => shell.close_reason !== 'closed_by_user' && !dismissed.has(shell.id) && shell.closed_at !== null && now - Date.parse(shell.closed_at) < CLOSED_SHOWN_MS);
    return [...open.sort((one, other) => Date.parse(one.opened_at) - Date.parse(other.opened_at)), ...ended];
  }, [shells.data, closed.data, dismissed, now]);

  // The first shell is the one in front: the person came here to use it.
  const current = sessions.find((shell) => shell.id === selected) ?? sessions[0] ?? null;

  const others = environment === undefined ? 0 : environment.threads.filter((one) => one.thread.id !== door.threadId).length + environment.threads_hidden;
  // The reported environment switch is one definite gate; policies whose effective result is
  // not reported here remain the service's decision. Existing shells and recordings stay useful.
  const reason = environment?.terminal === false ? 'environment_switch_off' : notAllowedReason(open.error);
  const openingBlocked = environment?.terminal === false ? t('terminal.refused.environment_switch_off') : false;

  if (shells.error !== null && shells.data === undefined) return <ErrorState compact className="h-full" error={shells.error} onRetry={() => void shells.refetch()} />;
  if (shells.data === undefined) return <SkeletonRows rows={3} />;

  const keptHours = (() => {
    const sample = shells.data.find((shell) => shell.closes_at !== null && shell.detached_at !== null);
    const closes = sample?.closes_at;
    const left = sample?.detached_at;
    if (closes === undefined || closes === null || left === undefined || left === null) return KEPT_HOURS_DEFAULT;
    return Math.max(1, Math.round((Date.parse(closes) - Date.parse(left)) / 3_600_000));
  })();

  const takeoverOf = (shell: TerminalSession): boolean => shell.state !== 'closed' && shell.attached && !here.has(shell.id);
  const scopeHelp = <HelpHint term={t('terminal.note.term')} what={others > 0 ? t('terminal.note.withOthers', { count: others }) : t('terminal.note.alone')} />;
  const create = <><IconButton action="newShell" size="sm" label={t('terminal.strip.new')} icon={<Plus aria-hidden />} disabled={openingBlocked !== false} title={openingBlocked || undefined} pending={open.isPending} onClick={() => open.mutate([])} />{scopeHelp}</>;

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="terminal-panel">
      {tab !== undefined && <ViewActions tab={tab}>{create}</ViewActions>}
      {sessions.length > 0 && <div className="flex items-center gap-1 border-b border-border px-1">
        <FoldingStrip className="min-w-0 flex-1" role="tablist" label={t('terminal.strip.label')} active={current?.id ?? ''} items={sessions.map((shell) => {
            const active = shell.id === current?.id;
            const label = shell.cwd === '' ? t('terminal.strip.root') : baseName(shell.cwd);
            const state =
              shell.state === 'closed' ? t('terminal.strip.closed') : takeoverOf(shell) ? t('terminal.strip.elsewhere') : shell.busy ? t('terminal.strip.running', { time: formatTimeOfDay(shell.last_io_at, format) }) : t('terminal.strip.prompt');
            const trip = live?.roundTripMs ?? 0;
            const slow = active && trip > SLOW_ROUND_TRIP_MS ? trip : null;
            const closeIt = () => {
              if (shell.state === 'closed') setDismissed((before) => new Set([...before, shell.id]));
              else close.mutate(shell.id);
            };
            const stateLabel = t('terminal.strip.stateLabel', { name: label, state });
            return { id: shell.id, label: stateLabel, onSelect: () => setSelected(shell.id),
              content: (
              // A strip of tabs holds tabs and nothing else for a screen reader: a shell is closed with Delete or from its menu,
              // and its cross is for the pointer alone, as the tabs of the workbench are.
              <ContextMenu key={shell.id} label={t('terminal.strip.menu', { name: label })} entries={[{ id: 'close', label: t('terminal.strip.close', { name: label }), danger: shell.state !== 'closed', onSelect: closeIt }]}>
                <div role="presentation" className={cx('flex min-w-0 max-w-full shrink-0 items-center rounded-t-md border-b-2', active ? 'border-accent bg-bg-subtle' : 'border-transparent')}>
                  <Tooltip content={[shell.cwd === '' ? t('terminal.strip.root') : shell.cwd, state, shell.state === 'closed' ? null : t('terminal.stay', { hours: keptHours })].filter((part) => part !== null).join(t('startFlow.separator'))}><button
                    type="button"
                    role="tab"
                    id={`shell-tab-${shell.id}`}
                    data-shell={shell.id}
                    aria-selected={active}
                    aria-controls={`shell-${shell.id}`}
                    aria-keyshortcuts="Delete"
                    tabIndex={active ? 0 : -1}
                    onClick={() => setSelected(shell.id)}
                    onKeyDown={(event) => {
                      if (event.key === 'Delete') {
                        event.preventDefault();
                        closeIt();
                      }
                    }}
                    className="focus-inset flex h-(--control-md) min-w-0 items-center px-2 text-left text-sm"
                  >
                    <span className={cx('flex min-w-0 items-center gap-1', shell.state === 'closed' ? 'text-text-muted line-through' : 'text-text')}>
                      <span className="truncate">{stateLabel}</span>
                      {slow !== null && (
                        <span className="inline-flex items-center gap-1 text-xs text-text-muted tabular-nums">
                          <TriangleAlert size={16} className="text-warning" aria-hidden />
                          {t('terminal.strip.roundTrip', { ms: slow })}
                        </span>
                      )}
                    </span>
                  </button></Tooltip>
                  <span aria-hidden className="flex">
                    <button type="button" tabIndex={-1} aria-label={t('terminal.strip.close', { name: label })} data-testid="shell-close" className="inline-flex size-(--control-sm) items-center justify-center rounded-md text-text-muted hover:bg-border hover:text-text" onClick={closeIt}>
                      <X size={16} aria-hidden />
                    </button>
                  </span>
                </div>
              </ContextMenu>
            ) };
          })} />
        {tab === undefined && create}
      </div>}
      {reason !== null && (
        <InlineAlert tone="warning" className="m-1" title={t('terminal.refused.title')}>
          {t(`terminal.refused.${reason}`, { machine: environment?.machine?.name ?? access.environment.name })}
        </InlineAlert>
      )}
      {sessions.length > 0 && open.error !== null && reason === null && limit === null && <ErrorAlert error={open.error} className="m-1" />}
      {sessions.length === 0 ? reason !== null || limit !== null ? null : open.error !== null ? <ErrorState compact className="flex-1" error={open.error} onRetry={tab === undefined ? () => open.mutate([]) : undefined} /> : (
        <EmptyState
          compact
          className="flex-1"
          icon={<SquareTerminal aria-hidden />}
          title={t('terminal.none')}
          description={t('terminal.noneAbout')}
          action={tab === undefined ?
            <div className="flex items-center gap-1"><Button action="create" variant="primary" icon={<Plus aria-hidden />} blocked={openingBlocked !== false} title={openingBlocked || undefined} pending={open.isPending} onClick={() => open.mutate([])}>
              {t('terminal.strip.new')}
            </Button>{scopeHelp}</div>
          : undefined}
        />
      ) : (
        <>
          <div className="min-h-0 flex-1" id={current === null ? undefined : `shell-${current.id}`} role="tabpanel">
            {current !== null && (
              <TerminalScreen
                key={current.id}
                session={current}
                firstTicket={first[current.id] ?? null}
                threadId={door.threadId}
                environmentName={access.environment.name}
                active={current.state === 'closed' || !takeoverOf(current)}
                onConnection={(made, id) => {
                  setHeld(made === null ? null : { id, connection: made });
                  // A shell this window lets go is nobody's here until it is attached again: another window may have it by then.
                  if (made === null) setHere((before) => new Set([...before].filter((one) => one !== id)));
                }}
              />
            )}
            {current !== null && current.state !== 'closed' && takeoverOf(current) && (
              <EmptyState
                compact
                className="h-full"
                icon={<SquareTerminal aria-hidden />}
                title={t('terminal.elsewhere.title')}
                description={t('terminal.elsewhere.about')}
                action={
                  <Button variant="primary" onClick={() => setHere((before) => new Set([...before, current.id]))}>
                    {t('terminal.elsewhere.take')}
                  </Button>
                }
              />
            )}
          </div>
        </>
      )}
      {limit !== null && (
        <LimitDialog
          limit={limit}
          environmentName={access.environment.name}
          onClose={() => setLimit(null)}
          onChosen={(ids) => open.mutate(ids)}
          pending={open.isPending}
          error={open.error !== null && atLimit(open.error) === null ? open.error : null}
        />
      )}
    </div>
  );
}

/** The Terminal view of a thread. */
export function TerminalView({ tab }: ViewProps) {
  const instance = useViewInstance();
  if (instance?.target?.kind === 'environment') return <EnvironmentTerminal tab={tab} />;
  return <ThreadViewFrame need="terminal.manage" icon={<SquareTerminal aria-hidden />}>{(access) => <TerminalPanel access={access} tab={tab} />}</ThreadViewFrame>;
}

function EnvironmentTerminal({ tab }: Pick<ViewProps, 'tab'>) {
  const { t } = useTranslation();
  const state = usePanelWorkspaceDoor();
  if (state.kind === 'error') return <ErrorState error={state.error} />;
  if (state.kind === 'loading') return <SkeletonRows rows={4} />;
  if (state.kind !== 'ready' || !state.access.mayShell) return <EmptyState icon={<SquareTerminal aria-hidden />} title={t('inspector.frame.noEnvironment')} />;
  return <TerminalPanel access={state.access} tab={tab} />;
}
