/** Title, compact state and measured header actions; shared state readers for the status bar.
 *
 * The head of the screen of a thread. The title can be renamed where it stands; the description is its
 * first line below it when present. The title and compact state share one line with Star, Notifications,
 * Share and the session menu; actions fold into that menu before they cut the title. On a phone the complete title
 * takes its own row above all the icon controls. The next turn's controls belong to the composer; runtime facts belong to
 * the status bar. The state reader below remains exported for that bar's richer clock and reason.
 * Every menu action still follows the caller's authority and changes only what it names.
 */
import { Hint } from '@/features/help/Hint';
import { cx } from '@/ui/cx';
import { useCompactControls } from '@/features/workbench/Narrow';
import { useViewInstance } from '@/features/workbench/instance';
import { useMutation, useQueries } from '@tanstack/react-query';
import { ArrowRightLeft, Box, Ellipsis, GitFork, History, LoaderCircle, MessageSquareDashed, Search, SquareTerminal } from 'lucide-react';
import { useContext, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AdminReachNotice } from '@/features/access/AdminReachNotice';
import { useMenuCommands } from '@/features/finder/menuCommands';
import { choiceOption } from '@/i18n/choices';
import { TagPicker } from '@/features/topic/TagPicker';
import { useStore } from 'zustand';
import { useThreadReadRef } from '@/realtime/threadReadContext';

import { client } from '@/api/client';
import { approvalQuery } from '@/api/approvals';
import { updateNode } from '@/api/nodes';
import { resumeAutomatic } from '@/api/threads';
import type { BackgroundCommand, BackgroundWork, Thread } from '@/generated/api-types';
import { openChange } from '@/features/change/target';
import { openGoalEditor } from '@/features/goal/editor';
import { isLive } from '@/features/goal/model';
import { openSideAsk } from '@/features/side-threads/ask';
import { SetUpThread } from '@/features/start/SetUpThread';
import { sideReadingAvailable, sideRefusal } from '@/features/side-threads/availability';
import { threadCapabilities } from '@/features/thread/cached';
import { SideThreadList } from '@/features/side-threads/SideChips';
import { SubagentChips } from '@/features/timeline/SubagentChips';
import { openAccess } from '@/features/access/AccessDialog';
import { StarButton } from '@/features/navigation/favorites';
import { useNodeMenu } from '@/features/navigation/NodeMenu';
import { openFind } from '@/features/timeline/FindInConversation';
import { CatchupRequest } from '@/features/inbox/away/Catchup';
import { placeOfNode } from '@/features/navigation/places';
import { DescriptionLine, DescriptionPanel } from '@/features/thread/Description';
import { SubscriptionBell } from '@/features/thread/SubscriptionBell';
import { useImmediateFork } from '@/features/thread/ImmediateFork';
import { RunningControl, RunningDialog } from '@/features/running/Running';
import { useRunning } from '@/features/running/live';
import { BackgroundWorkLine, NativeBackgroundCommands } from '@/features/running/BackgroundWork';
import type { BackgroundEndTarget } from '@/features/running/review';
import { SettingsEditor } from '@/features/settings/Editor';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { restorePromptFocus } from '@/ui/PromptScope';
import { MoveToTopic } from '@/features/thread-move/MoveToTopic';
import { useTimelinePreferences } from '@/features/timeline/preferences';
import { useTitlePriority } from '@/features/thread/titlePriority';
import { DetailControl } from '@/features/thread/DetailControl';
import { ThreadNowChip, useProvidedThreadNow } from '@/features/thread/NowLine';
import type { WorkDetail } from '@/features/timeline/rows';
import { isAgentHistoryRemoving, waitingWords } from '@/features/timeline/waiting';
import { holds } from '@/lib/permissions';
import { useRunningClock } from '@/lib/time';
import { turnActiveMs, turnWaiting } from '@/features/timeline/turnTime';
import { approvalAtItem } from '@/features/approvals/current';
import { runningTurn, threadReadOnly, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { ActionIcon, actionName } from '@/ui/actions';
import { choiceName } from '@/ui/Choice';
import { Dialog } from '@/ui/Dialog';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Popover } from '@/ui/Popover';
import { PresenceStack } from '@/ui/PresenceStack';
import { StateChip } from '@/ui/StateChip';
import { StateLine } from '@/ui/StateLine';
import { threadDisplayState, useThreadStateLabel, ThreadStateIcon } from '@/ui/ThreadState';
import { Tooltip } from '@/ui/Tooltip';
import { UserChip } from '@/ui/UserChip';
import { toast } from '@/ui/Toast';
import { StatusPlace } from '@/features/status-items/StatusPlace';
import { openThreadChanges } from '@/features/inspector/reviewTarget';
import { useActionKeys } from '@/shell/keys';

/** Thread acts that keep direct icons while the title has room. */
const HEADER_ACTIONS = new Set(['change-fork-now', 'change-fork', 'change-switch', 'side-ask']);
const COMMAND_ICONS = { 'change-fork-now': GitFork, 'change-fork': GitFork, 'change-switch': ArrowRightLeft, 'side-ask': MessageSquareDashed, 'change-move': Box, running: SquareTerminal };

/** A running time as minutes and seconds, or with hours once it is that long: "2:14". */
export function clock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1_000));
  const hours = Math.floor(total / 3_600);
  const minutes = Math.floor((total % 3_600) / 60);
  const seconds = total % 60;
  const two = (value: number) => String(value).padStart(2, '0');
  return hours > 0 ? `${hours}:${two(minutes)}:${two(seconds)}` : `${minutes}:${two(seconds)}`;
}

function Title({ thread, mayEdit }: { thread: Thread; mayEdit: boolean }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(thread.name);
  const save = useMutation({
    mutationFn: (name: string) => updateNode(thread.id, { name, expected_revision: thread.revision }),
    onSuccess: (node) => {
      if (node.kind === 'thread') threadWriter.apply(thread.id, { thread: node });
      if (thread.title_source === 'auto') toast({ tone: 'info', title: t('thread.header.titleReplaces') });
      setEditing(false);
    },
  });
  const commit = () => {
    const name = value.trim();
    if (name === '' || name === thread.name) setEditing(false);
    else save.mutate(name);
  };
  if (editing) {
    return (
      <form
        className="flex min-w-0 flex-col gap-1"
        onSubmit={(event) => {
          event.preventDefault();
          commit();
        }}
      >
        <Input
          label={t('thread.header.titleField')}
          hideLabel
          value={value}
          autoFocus
          disabled={save.isPending}
          onChange={(event) => setValue(event.target.value)}
          onBlur={() => !save.isPending && commit()}
          onKeyDown={(event) => {
            if (event.key === 'Escape') {
              setValue(thread.name);
              setEditing(false);
            }
          }}
          fieldClassName="w-(--chip-width) max-w-full"
        />
        {save.error !== null && <ErrorAlert error={save.error} />}
      </form>
    );
  }
  return mayEdit ? (
    <button
      type="button"
      aria-label={t('thread.header.rename')}
      onClick={() => {
        setValue(thread.name);
        setEditing(true);
      }}
      className="group/title flex min-w-0 max-w-full items-start gap-1 rounded-md text-left"
    >
      <h1 className="min-w-0 wrap-anywhere whitespace-normal text-lg font-semibold text-text sm:truncate">{thread.name}</h1>
      <span className="inline-flex h-7 shrink-0 items-center"><ActionIcon action="rename" className="text-text-subtle opacity-0 transition-opacity group-focus-visible/title:opacity-100 group-hover/title:opacity-100" /></span>
    </button>
  ) : (
    <h1 className="min-w-0 wrap-anywhere whitespace-normal text-lg font-semibold text-text sm:truncate">{thread.name}</h1>
  );
}

/** The live state, elapsed clock and waiting reason, shared with the workbench status bar. */
export function useThreadState(thread: Thread) {
  const { t } = useTranslation();
  const turn = useThreadPart(thread.id, runningTurn);
  const now = useRunningClock(turn !== null && thread.state === 'running' && !turnWaiting(turn));
  const words = waitingWords(thread, t);
  const state = threadDisplayState(thread.display);
  const displayLabel = useThreadStateLabel(thread.display);
  const label = isAgentHistoryRemoving(thread) && !thread.archived ? t('agentHistory.removing') : state === 'running' && thread.state === 'running' && turn !== null && turn.started_at !== null
    ? t('thread.header.elapsed', { state: t('state.running'), time: clock(turnActiveMs(turn, now) ?? 0) })
    : displayLabel ?? t('everything.unknown');
  return { state, label, words };
}

/** Compact in the header; the full state and its reason remain available to other surfaces. */
export function StateFact({ thread, compact = false }: { thread: Thread; compact?: boolean }) {
  const { t } = useTranslation();
  const phone = useCompactControls();
  const currentNow = useProvidedThreadNow();
  const { state, label, words } = useThreadState(thread);
  const items = useThreadPart(thread.id, (view) => view.items);
  const pending = useMemo(() => [...items.values()].flatMap((item) => item.kind === 'approval' && item.status === 'pending' ? [item] : []), [items]);
  // An inline snapshot already answers who may decide; older items need the same read as the card.
  // A confirmed POST stops addressing its former decider before the next thread frame arrives.
  const observed = useQueries({ queries: pending.map((item) => ({ ...approvalQuery(item.payload.approval_id), enabled: state === 'waiting_approval' && approvalAtItem(item, undefined) === undefined })) });
  const approvals = pending.map((item, index) => approvalAtItem(item, observed[index]?.data));
  const canAnswer = approvals.some((current) => current?.state === 'pending' && current.can_decide);
  const answerUnknown = pending.length < thread.pending_approvals || approvals.some((current) => current === undefined);
  if (state === undefined) return <ThreadStateIcon display={null} />;
  if (compact && phone && currentNow?.now.thread?.id === thread.id && currentNow.now.active) return <ThreadNowChip state={state} />;
  if (compact) {
    const short = isAgentHistoryRemoving(thread) && !thread.archived ? label
      : state === 'blocked' && thread.display.blocked_reason === 'unbound' ? t('thread.header.needsSetup')
      : state === 'waiting_approval' ? (thread.display.waiting_inputs > 0 ? t('day100.views.needsAttention') : t(canAnswer ? 'composer.row.waitingForYou' : answerUnknown ? 'thread.header.waitingOnApproval' : 'thread.header.waitingForAnswer')) : t(`state.${state}`);
    return <Tooltip content={label}><span role="img" aria-label={label} data-state={state} className="inline-flex shrink-0"><StateChip state={state} label={short} /></span></Tooltip>;
  }
  const chip = <span role="img" aria-label={label}><StateChip state={state} label={label} /></span>;
  if (words === null) return <span data-state={state}>{chip}</span>;
  return <span data-state={state} className="inline-flex items-center gap-1">
    {chip}
    <Popover label={actionName('statusDetails')} trigger={<IconButton size="sm" action="statusDetails" label={actionName('statusDetails')} />}>
      <p className="max-w-(--container-form) text-sm text-text">{words}</p>
    </Popover>
  </span>;
}

export function ThreadHeader({ thread, onRevealItem, recoverySwitchVisible = false }: { thread: Thread; onRevealItem?: ((itemId: string) => void) | undefined; recoverySwitchVisible?: boolean }) {
  const compact = useCompactControls();
  const instance = useViewInstance();
  const read = useThreadReadRef(thread.id);
  const { t } = useTranslation();
  const meId = useStore(client.session, (state) => state.userId);
  const readerVersion = useStore(client.session, (state) => state.readerVersion);
  const opening = useContext(ViewOpeningContext);
  const readOnly = useThreadPart(thread.id, threadReadOnly);
  const runningScope = JSON.stringify([thread.id, opening === undefined ? 'outside' : opening, meId, readerVersion, readOnly]);
  const preferences = useTimelinePreferences(thread.id);
  const presence = useThreadPart(thread.id, (view) => view.presence);
  const turns = useThreadPart(thread.id, (view) => view.turns);
  const segments = useThreadPart(thread.id, (view) => view.segments);
  const importedHistory = useMemo(() => segments.some((segment) => segment.agent === null && segment.import_fidelity !== null), [segments]);
  const [settingUp, setSettingUp] = useState<string | null>(null);
  const [describing, setDescribing] = useState(false);
  const [tagging, setTagging] = useState(false);
  const [editingDescription, setEditingDescription] = useState(false);
  const [catchupOpen, setCatchupOpen] = useState(false);
  const [runningFor, setRunningFor] = useState<string | null>(null);
  const [endTarget, setEndTarget] = useState<BackgroundEndTarget | null>(null);
  const [wakeSettingsFor, setWakeSettingsFor] = useState<string | null>(null);
  const runningOpener = useRef<HTMLElement | null>(null);
  const actionsButton = useRef<HTMLButtonElement>(null);
  const background = useRunning(thread.id, !readOnly);
  if (runningFor !== null && (runningFor !== runningScope || background.closed)) { setRunningFor(null); setEndTarget(null); }
  if (wakeSettingsFor !== null && (wakeSettingsFor !== runningScope || readOnly || thread.archived || !holds(thread.access, 'thread.manage'))) setWakeSettingsFor(null);
  const beginRunning = (ending: BackgroundEndTarget | null) => {
    const target = document.activeElement;
    runningOpener.current = target instanceof HTMLElement && target !== document.body && target !== document.documentElement ? target : null;
    setEndTarget(ending);
    setRunningFor(runningScope);
  };
  const openRunning = () => beginRunning(null);
  const endTask = (task: BackgroundWork) => beginRunning({ task: { id: task.id, item_id: task.item_id, turn_id: task.turn_id } });
  const endCommand = (command: BackgroundCommand) => beginRunning({ command: { id: command.id, segment_id: command.segment_id, item_id: command.item_id, turn_id: command.turn_id } });
  const closeRunning = () => {
    setRunningFor(null);
    requestAnimationFrame(() => {
      const target = runningOpener.current;
      if (target !== null && restorePromptFocus(target)) target.focus();
      else if (actionsButton.current !== null && restorePromptFocus(actionsButton.current)) actionsButton.current.focus();
    });
  };
  const runningEntry = { id: 'running', label: t('running.title'), onSelect: openRunning };
  const changesTurn = thread.current_turn_id ?? [...turns.values()].reduce<{ ordinal: number; id: string } | null>((held, turn) => held === null || turn.ordinal > held.ordinal ? turn : held, null)?.id;
  const changesKeys = useActionKeys('thread-changes');
  const changesEntry: MenuEntry = { id: 'changes', label: t('views.changes.title'), ...(changesKeys === undefined ? {} : { shortcut: changesKeys }),
    disabled: thread.environment === null ? t('inspector.frame.noEnvironment') : !thread.environment_permissions.includes('files.view') ? t('files.permission.view') : false,
    onSelect: () => openThreadChanges(thread.id, changesTurn) };
  const [sidesOpen, setSidesOpen] = useState(false);
  const [peopleOpen, setPeopleOpen] = useState(false);
  const [movingTopic, setMovingTopic] = useState(false);
  const [notificationsOpen, setNotificationsOpen] = useState(false);
  const mayManage = holds(thread.access, 'thread.manage');
  const setupReason = readOnly ? t('composer.row.readOnly')
    : !mayManage ? t('composer.row.bindingReadOnly', { who: thread.owner.display_name })
    : thread.lock !== null ? t('threadMove.jobMustEnd') : null;
  const capabilities = threadCapabilities(thread);
  const mayDescribe = mayManage && !thread.archived;
  const mayDrive = holds(thread.access, 'thread.interact') && !thread.archived;
  const wake = readOnly ? null : (background.closed ? null : background.roster?.wake) ?? thread.background ?? null;
  const goal = thread.goal ?? null;
  const goalEditable = goal !== null && (isLive(goal) || goal.status === 'stopped');
  const place = placeOfNode(thread);
  const nodeEntries = useNodeMenu(place, { where: 'screen', node: thread });

  const auto = useMutation({
    mutationFn: (change: { field: 'title'; action: 'resume' | 'refresh' }) => resumeAutomatic(thread.id, change.field, change.action),
    onSuccess: (row) => {
      threadWriter.apply(thread.id, { thread: row });
    },
    onError: () => toast({ tone: 'danger', title: t('thread.header.notSaved') }),
  });

  const quickFork = useImmediateFork(thread);
  const automatic: MenuEntry[] = [];
  if (mayManage) {
    automatic.push(
      thread.title_source === 'user'
        ? { id: 'resume-title', label: t('thread.header.resumeAuto'), onSelect: () => auto.mutate({ field: 'title', action: 'resume' }) }
        : { id: 'refresh-title', label: t('thread.header.refreshAuto'), onSelect: () => auto.mutate({ field: 'title', action: 'refresh' }) },
    );
  }
  // Advanced forks, switches and moves share the planner; immediate built-in forks keep their Private creation.
  const change: MenuEntry[] = [];
  const asideRefusal = sideRefusal(thread, t);
  if (thread.side_of === null && !thread.archived) {
    const target = { kind: 'thread' as const, id: thread.id, name: thread.name };
    if (thread.lock !== null) {
      const job = thread.lock.id;
      change.push({ id: 'change-progress', label: t('change.menu.progress'), icon: <LoaderCircle aria-hidden />, onSelect: () => openChange(target, { kind: 'progress', job }) });
    } else {
      // Immediate built-in copies retain the program; the advanced action chooses its native target.
      if (capabilities?.fork === true) {
        change.push({ id: 'change-fork-now', label: t('duplicate.now'), icon: <GitFork aria-hidden />, disabled: quickFork.pending ? t('change.forkNow.making') : false, onSelect: () => quickFork.start() });
        change.push({ id: 'change-fork', label: t('duplicate.to'), icon: <GitFork aria-hidden />, onSelect: () => openChange(target, { kind: 'fork' }) });
      }
      if (holds(thread.access, 'thread.interact') && sideReadingAvailable(thread)) change.push({ id: 'side-ask', label: t('side.menu.ask'), icon: <MessageSquareDashed aria-hidden />, disabled: asideRefusal, onSelect: () => openSideAsk({ threadId: thread.id, threadName: thread.name, toTurn: null, text: '', read }) });
      if (mayManage) change.push({ id: 'change-switch', label: t('change.menu.switch'), icon: <ArrowRightLeft aria-hidden />, onSelect: () => openChange(target, { kind: 'switch', focus: 'provider' }) });
      if (capabilities?.place === true) change.push({ id: 'change-move', label: t('change.menu.move'), icon: <Box aria-hidden />, onSelect: () => openChange(target, { kind: 'switch', focus: 'environment' }) });
    }
  }
  const inHeader = (entry: MenuEntry) => HEADER_ACTIONS.has(entry.id);
  const sourceChanges = readOnly ? change.map((entry) => 'onSelect' in entry && entry.id !== 'change-progress'
    ? { ...entry, disabled: t('messageVersions.activeActions') } : entry) : change;
  useMenuCommands([...sourceChanges, ...(instance === null || instance.focusedContent ? [runningEntry] : [])], { prefix: 'thread', name: thread.name, icons: COMMAND_ICONS });
  const headerActions = sourceChanges.flatMap((entry) => inHeader(entry) && !(recoverySwitchVisible && entry.id === 'change-switch') && 'onSelect' in entry ? [entry] : []);
  const headerRef = useRef<HTMLElement>(null);
  const titleLayout = useTitlePriority(headerRef, compact, thread.name, headerActions.map((entry) => entry.id).join('|'));
  const foldQuick = titleLayout.actions !== 'all';
  const foldPrimary = titleLayout.actions === 'menu';
  const findEntry: MenuEntry = { id: 'find', label: t('search.find.command'), icon: <Search aria-hidden />, onSelect: () => openFind(thread.id, instance?.id) };
  const notificationsEntry: MenuEntry = { id: 'notifications', label: t('thread.bell.label'), onSelect: () => setNotificationsOpen(true) };
  const share = () => openAccess({ kind: 'thread' as const, id: thread.id, name: thread.name });
  // Each folded action keeps its original handler, authority, pending state and refusal.
  const otherChanges = foldQuick ? sourceChanges.filter((entry) => !headerActions.some((action) => action.id === entry.id)) : sourceChanges;
  const remainingNode = nodeEntries.filter((entry) => entry.id !== 'duplicate' && entry.id !== 'duplicate-to');
  const exported = remainingNode.filter((entry) => entry.id.startsWith('export'));
  const endings = remainingNode.filter((entry) => ['archive', 'unarchive', 'delete'].includes(entry.id));
  const organized = remainingNode.filter((entry) => !entry.id.startsWith('export') && !['archive', 'unarchive', 'delete'].includes(entry.id) && entry.type !== 'separator' && !(foldPrimary && ['star', 'unstar', 'share'].includes(entry.id)));
  const entries: MenuEntry[] = [
    ...(foldQuick ? [...headerActions, findEntry] : []),
    ...(foldPrimary ? [...remainingNode.filter((entry) => ['star', 'unstar'].includes(entry.id)), notificationsEntry, { id: 'share', label: actionName('share'), icon: <ActionIcon action="share" />, onSelect: share } as MenuEntry] : []),
    ...(foldQuick ? [{ type: 'separator', id: 'header-separator' } as MenuEntry] : []),
    ...(thread.description.trim() !== '' || mayDescribe ? [{ id: 'description', label: t(thread.description.trim() === '' ? 'thread.description.add' : mayDescribe ? 'thread.description.edit' : 'thread.description.expand'), onSelect: () => { setEditingDescription(mayDescribe); setDescribing(true); } } as MenuEntry] : []),
    ...(thread.archived || thread.side_of !== null ? [] : [{ id: 'goal', label: t(goalEditable ? 'goals.act.edit' : 'goals.menu.set'), disabled: !mayDrive ? t('goals.act.driveOnly') : false, onSelect: () => openGoalEditor(thread.id) } as MenuEntry]),
    { type: 'sub', id: 'organize', label: t('thread.header.organize'), entries: [
      { id: 'tags', label: t('placeTags.title'), onSelect: () => setTagging(true) },
      ...(mayManage && thread.side_of === null && !thread.archived ? [{ id: 'move-topic', label: t('threadMove.menu'), disabled: thread.lock !== null ? t('threadMove.jobMustEnd') : false, onSelect: () => setMovingTopic(true) } as MenuEntry] : []),
      ...organized,
    ] },
    {
      type: 'sub',
      id: 'view',
      label: t('thread.header.view'),
      entries: [
        ...(!foldQuick ? [findEntry] : []),
        ...(!foldPrimary ? [notificationsEntry] : []),
        runningEntry,
        changesEntry,
        ...((thread.sides?.open ?? 0) > 0 ? [{ id: 'side-threads', label: t('side.chip.text', { count: thread.sides?.open ?? 0 }), onSelect: () => setSidesOpen(true) } as MenuEntry] : []),
        ...(presence.some((entry) => entry.user.id !== meId) ? [{ id: 'presence', label: t('ui.presence.here', { names: presence.filter((entry) => entry.user.id !== meId).map((entry) => entry.user.display_name).join(', ') }), onSelect: () => setPeopleOpen(true) } as MenuEntry] : []),
        { id: 'catchup', label: t('away.inbox.catchup'), icon: <History aria-hidden />, onSelect: () => setCatchupOpen(true) },
        {
          type: 'radio',
          id: 'detail',
          label: t('thread.header.workDetail'),
          value: preferences.detail,
          onValueChange: (value) => preferences.setDetail(value as WorkDetail),
          options: [
            { value: 'steps', ...choiceOption(t, 'workDetail.steps') },
            { value: 'summary', ...choiceOption(t, 'workDetail.summary') },
            { value: 'everything', ...choiceOption(t, 'workDetail.everything') },
          ],
        },
        { type: 'check', id: 'hide-history', ...choiceOption(t, 'navigationShow.hide_history'), description: t('thread.header.hideHistory'), checked: preferences.hideHistory, onCheckedChange: preferences.setHideHistory },
        ...(automatic.length > 0 ? [{ type: 'separator', id: 'auto-separator' } as MenuEntry, ...automatic] : []),
      ],
    },
    ...(otherChanges.length > 0 ? [{ type: 'sub', id: 'placement', label: t('thread.header.placement'), entries: otherChanges } as MenuEntry] : []),
    ...exported,
    ...(endings.length > 0 ? [{ type: 'separator', id: 'node-separator' } as MenuEntry, ...endings] : []),
  ];

  return (
    <><StatusPlace place="header" threadId={thread.id} className="shrink-0"><header ref={headerRef} className="flex shrink-0 flex-col gap-1 border-b border-border px-4 py-2" aria-label={t('thread.header.label')} data-thread-header data-thread-actions-mode={titleLayout.actions}>
      <div className="flex min-w-0 flex-wrap items-center gap-2" data-thread-title-row>
        <div className={cx('flex min-w-0 flex-col gap-1', compact ? 'basis-full' : 'basis-0 flex-1')}>
          <div className="flex min-w-0 flex-wrap items-center gap-2" data-thread-identity><div className={cx('min-w-0 flex-1', (compact || titleLayout.stacked) && 'basis-full')}><Title thread={thread} mayEdit={mayManage && !thread.archived} /></div><StateFact thread={thread} compact /><DetailControl preferences={preferences} /><RunningControl roster={background.roster} closed={background.closed} opened={runningFor === runningScope} onOpen={openRunning} /><Hint id="thread-states" popover /></div>
          <BackgroundWorkLine status={wake} endUnavailableReason={background.roster?.supported === false ? background.roster.reason : null} showHeldCompletions={readOnly} canManage={mayManage && !thread.archived && !readOnly} mayEnd={mayDrive && !readOnly && !background.closed} onSetting={() => setWakeSettingsFor(runningScope)} onEnd={endTask}>
            <NativeBackgroundCommands roster={readOnly || background.closed ? null : background.roster} represented={wake?.tasks ?? []} mayEnd={mayDrive && !readOnly && !background.closed} onEnd={endCommand} />
            <SubagentChips threadId={thread.id} onReveal={onRevealItem} {...(wake === null ? {} : { tasks: wake.tasks })} mayEnd={mayDrive && !readOnly && !background.closed} onEnd={endTask} />
          </BackgroundWorkLine>
        </div>
        <div className={cx('relative flex shrink-0 items-center gap-1', compact ? 'w-full' : 'w-auto')} data-thread-actions-row>
          <div role="group" aria-label={t('thread.header.quickActions')} data-thread-quick-actions aria-hidden={foldQuick || undefined} inert={foldQuick} className={cx('flex w-max shrink-0 items-center gap-1', foldQuick && 'pointer-events-none invisible absolute right-0 top-0')}>
            {headerActions.map((entry) => <IconButton key={entry.id} size="sm" label={choiceName(entry)} icon={entry.icon} blocked={Boolean(entry.disabled)}
              reason={typeof entry.disabled === 'string' ? entry.disabled : undefined} onClick={entry.onSelect} />)}
            <IconButton size="sm" icon={<Search aria-hidden />} label={t('search.find.command')} onClick={() => openFind(thread.id, instance?.id)} />
          </div>
          <div data-thread-primary-actions aria-hidden={foldPrimary || undefined} inert={foldPrimary} className={cx('flex w-max shrink-0 items-center gap-1', compact && 'ml-auto', foldPrimary && 'pointer-events-none invisible absolute right-0 top-0')}>
          <StarButton place={place} />
          <SubscriptionBell thread={thread} />
          <IconButton size="sm" action="share" label={actionName('share')} onClick={share} />
          </div>
          <Menu label={actionName('actions', thread.name)} align="end" entries={entries} trigger={<IconButton ref={actionsButton} data-thread-more size="sm" label={actionName('actions', thread.name)} icon={<Ellipsis aria-hidden />} />} />
        </div>
      </div>
      {importedHistory && <StateLine compact icon={<History aria-hidden />} message={t('thread.header.importedHistory')}
        action={thread.binding === null && thread.program?.id !== 'orch' && !thread.archived && thread.side_of === null ? <Tooltip content={setupReason}>
          <Button size="sm" blocked={setupReason !== null} onClick={() => setSettingUp(thread.id)}>{t('thread.header.chooseAgentProvider')}</Button>
        </Tooltip> : undefined} />}
      <AdminReachNotice of={thread.access.admin_access?.place} private={thread.page === 'restricted'} object={{ kind: 'thread', id: thread.id }} />
      {thread.description.trim() !== '' && <DescriptionLine thread={thread} open={describing} onToggle={() => { setEditingDescription(false); setDescribing(true); }} mayEdit={false} />}
      {wakeSettingsFor === runningScope && <Dialog open onOpenChange={(open) => { if (!open) setWakeSettingsFor(null); }} title={t('backgroundWake.settingTitle')}>
        <SettingsEditor target={{ kind: 'node', id: thread.id }} scope="thread" only={['threads.background_wake']} compact place={{ name: thread.name, owner: thread.owner, access: thread.access, kind: 'thread' }} />
      </Dialog>}
      {describing && <Dialog open onOpenChange={setDescribing} title={t('thread.description.field')}><DescriptionPanel key={`${thread.description}:${editingDescription}`} thread={thread} initialEdit={editingDescription} mayEdit={mayManage && !thread.archived} onClose={() => setDescribing(false)} /></Dialog>}
      {tagging && <Dialog open onOpenChange={setTagging} title={t('placeTags.title')}><TagPicker place={thread} allowed={holds(thread.access, 'thread.interact') && !thread.archived} /></Dialog>}
      {catchupOpen && <CatchupRequest threadId={thread.id} open onOpenChange={setCatchupOpen} />}
    </header></StatusPlace>
    {settingUp === thread.id && thread.binding === null && thread.program?.id !== 'orch' && !thread.archived && thread.side_of === null && setupReason === null && <SetUpThread thread={thread} onClose={() => setSettingUp(null)} />}
    {notificationsOpen && <Dialog open onOpenChange={setNotificationsOpen} title={t('thread.bell.label')}><SubscriptionBell thread={thread} /></Dialog>}
    {runningFor === runningScope && !background.closed && <RunningDialog key={JSON.stringify([runningScope, endTarget])} thread={thread.id} close={closeRunning} {...(endTarget === null ? {} : { endTarget })} />}
    {sidesOpen && <Dialog open onOpenChange={setSidesOpen} title={t('side.chip.label')}><SideThreadList thread={thread} onOpened={() => setSidesOpen(false)} onRevealItem={(id) => onRevealItem?.(id)} /></Dialog>}
    {peopleOpen && <Dialog open onOpenChange={setPeopleOpen} title={t('composer.row.looking')}><PresenceStack users={presence} without={meId} /><ul className="flex flex-col gap-2">{presence.filter((entry) => entry.user.id !== meId).map((entry) => <li key={entry.user.id}><UserChip user={entry.user} /></li>)}</ul></Dialog>}
    {quickFork.confirmation}
    {movingTopic && <MoveToTopic key={thread.id} thread={thread} close={() => setMovingTopic(false)} />}
    </>
  );
}
