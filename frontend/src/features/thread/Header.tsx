/** Title, compact state and visible thread actions that wrap; shared state readers for the status bar.
 *
 * The head of the screen of a thread. The title can be renamed where it stands; the description is its
 * first line below it when present. The title and compact state share one line with Star, Notifications,
 * Share and the session menu; frequent acts use one row of icons. On a phone the complete title
 * takes its own row above all the icon controls. The next turn's controls belong to the composer; runtime facts belong to
 * the status bar. The state reader below remains exported for that bar's richer clock and reason.
 * Every menu action still follows the caller's authority and changes only what it names.
 */
import { Hint } from '@/features/help/Hint';
import { cx } from '@/ui/cx';
import { useCompactControls } from '@/features/workbench/Narrow';
import { useViewInstance } from '@/features/workbench/instance';
import { useMutation, useQueries } from '@tanstack/react-query';
import { ArrowRightLeft, Box, Ellipsis, GitFork, History, LoaderCircle, MessageSquareDashed, Search } from 'lucide-react';
import { useMemo, useState } from 'react';
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
import type { Thread } from '@/generated/api-types';
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
import { useForkNow } from '@/features/thread/forkNow';
import { RunningDialog } from '@/features/running/Running';
import { MoveToTopic } from '@/features/thread-move/MoveToTopic';
import { useTimelinePreferences } from '@/features/timeline/preferences';
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
import { threadDisplayState, useThreadStateLabel, ThreadStateIcon, threadIconFacts } from '@/ui/ThreadState';
import { Tooltip } from '@/ui/Tooltip';
import { UserChip } from '@/ui/UserChip';
import { toast } from '@/ui/Toast';
import { StatusPlace } from '@/features/status-items/StatusPlace';

/** Frequent thread acts remain directly reachable, rather than being repeated in the session menu. */
const HEADER_ACTIONS = new Set(['change-fork-now', 'change-fork', 'change-switch', 'side-ask']);
const COMMAND_ICONS = { 'change-fork-now': GitFork, 'change-fork': GitFork, 'change-switch': ArrowRightLeft, 'side-ask': MessageSquareDashed, 'change-move': Box };

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
      className="group/title flex min-w-0 max-w-full items-center gap-1 rounded-md text-left"
    >
      <h1 className="min-w-0 wrap-anywhere whitespace-normal text-lg font-semibold text-text sm:truncate">{thread.name}</h1>
      <ActionIcon action="rename" className="shrink-0 text-text-subtle opacity-0 transition-opacity group-focus-visible/title:opacity-100 group-hover/title:opacity-100" />
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
  const { state, label, words } = useThreadState(thread);
  const items = useThreadPart(thread.id, (view) => view.items);
  const pending = useMemo(() => [...items.values()].flatMap((item) => item.kind === 'approval' && item.status === 'pending' ? [item] : []), [items]);
  // Observe the same cache as the card for every request, including inline cards. A confirmed POST
  // stops addressing its former decider at once; thread state itself still comes from the live feed.
  const observed = useQueries({ queries: pending.map((item) => ({ ...approvalQuery(item.payload.approval_id), enabled: false })) });
  const canAnswer = pending.some((item, index) => {
    const current = approvalAtItem(item, observed[index]?.data);
    return current?.state === 'pending' && current.can_decide;
  });
  if (threadIconFacts(thread) === undefined) return <ThreadStateIcon display={undefined} />;
  if (compact) {
    const short = isAgentHistoryRemoving(thread) && !thread.archived ? label
      : state === 'blocked' && thread.display.blocked_reason === 'unbound' ? t('thread.header.needsSetup')
      : state === 'waiting_approval' ? (thread.display.waiting_inputs > 0 ? t('day100.views.needsAttention') : t(canAnswer ? 'composer.row.waitingForYou' : 'thread.header.waitingForAnswer')) : t(`state.${state}`);
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

export function ThreadHeader({ thread, onRevealItem }: { thread: Thread; onRevealItem?: ((itemId: string) => void) | undefined }) {
  const compact = useCompactControls();
  const instance = useViewInstance();
  const read = useThreadReadRef(thread.id);
  const { t } = useTranslation();
  const meId = useStore(client.session, (state) => state.userId);
  const preferences = useTimelinePreferences();
  const presence = useThreadPart(thread.id, (view) => view.presence);
  const segments = useThreadPart(thread.id, (view) => view.segments);
  const importedHistory = useMemo(() => segments.some((segment) => segment.agent === null && segment.import_fidelity !== null), [segments]);
  const readOnly = useThreadPart(thread.id, threadReadOnly);
  const [settingUp, setSettingUp] = useState<string | null>(null);
  const [describing, setDescribing] = useState(false);
  const [tagging, setTagging] = useState(false);
  const [editingDescription, setEditingDescription] = useState(false);
  const [catchupOpen, setCatchupOpen] = useState(false);
  const [runningOpen, setRunningOpen] = useState(false);
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
  const goal = thread.goal ?? null;
  const goalEditable = goal !== null && (isLive(goal) || goal.status === 'stopped');
  const place = placeOfNode(thread);
  const nodeEntries = useNodeMenu(place, { where: 'screen', node: thread });

  const auto = useMutation({
    mutationFn: (change: { field: 'title' | 'description'; action: 'resume' | 'refresh' }) => resumeAutomatic(thread.id, change.field, change.action),
    onSuccess: (row) => {
      threadWriter.apply(thread.id, { thread: row });
    },
    onError: () => toast({ tone: 'danger', title: t('thread.header.notSaved') }),
  });

  const quickFork = useForkNow(thread.id);
  const automatic: MenuEntry[] = [];
  if (mayManage) {
    automatic.push(
      thread.title_source === 'user'
        ? { id: 'resume-title', label: t('thread.header.resumeAuto'), onSelect: () => auto.mutate({ field: 'title', action: 'resume' }) }
        : { id: 'refresh-title', label: t('thread.header.refreshAuto'), onSelect: () => auto.mutate({ field: 'title', action: 'refresh' }) },
      thread.description_source === 'user'
        ? { id: 'resume-description', label: t('thread.description.resume'), onSelect: () => auto.mutate({ field: 'description', action: 'resume' }) }
        : { id: 'refresh-description', label: t('thread.description.refresh'), onSelect: () => auto.mutate({ field: 'description', action: 'refresh' }) },
    );
  }
  // Fork, switch and move are one wizard (the thread's own side threads take none of them); a thread that is held shows how far that has come.
  const change: MenuEntry[] = [];
  const asideRefusal = sideRefusal(thread, t);
  if (thread.side_of === null && !thread.archived) {
    const target = { kind: 'thread' as const, id: thread.id, name: thread.name };
    if (thread.lock !== null) {
      const job = thread.lock.id;
      change.push({ id: 'change-progress', label: t('change.menu.progress'), icon: <LoaderCircle aria-hidden />, onSelect: () => openChange(target, { kind: 'progress', job }) });
    } else {
      // "Fork" is one request with nothing to choose, opening the new thread; "Fork to…" plans one that changes where or on what it runs.
      if (capabilities?.fork === true) {
        change.push({ id: 'change-fork-now', label: t('change.menu.forkNow'), icon: <GitFork aria-hidden />, disabled: quickFork.pending ? t('change.forkNow.making') : false, onSelect: () => quickFork.start() });
        change.push({ id: 'change-fork', label: t('change.menu.fork'), icon: <GitFork aria-hidden />, onSelect: () => openChange(target, { kind: 'fork' }) });
      }
      if (holds(thread.access, 'thread.interact') && sideReadingAvailable(thread)) change.push({ id: 'side-ask', label: t('side.menu.ask'), icon: <MessageSquareDashed aria-hidden />, disabled: asideRefusal, onSelect: () => openSideAsk({ threadId: thread.id, threadName: thread.name, toTurn: null, text: '', read }) });
      if (mayManage) change.push({ id: 'change-switch', label: t('change.menu.switch'), icon: <ArrowRightLeft aria-hidden />, onSelect: () => openChange(target, { kind: 'switch', focus: 'provider' }) });
      if (capabilities?.place === true) change.push({ id: 'change-move', label: t('change.menu.move'), icon: <Box aria-hidden />, onSelect: () => openChange(target, { kind: 'switch', focus: 'environment' }) });
    }
  }
  const inHeader = (entry: MenuEntry) => HEADER_ACTIONS.has(entry.id);
  const sourceChanges = readOnly ? change.map((entry) => 'onSelect' in entry && entry.id !== 'change-progress'
    ? { ...entry, disabled: t('messageVersions.activeActions') } : entry) : change;
  useMenuCommands(sourceChanges, { prefix: 'thread', name: thread.name, icons: COMMAND_ICONS });
  const headerActions = sourceChanges.flatMap((entry) => inHeader(entry) && 'onSelect' in entry ? [entry] : []);
  // Compact icons and named menu entries provide the same actions.
  const otherChanges = sourceChanges;
  const remainingNode = nodeEntries;
  const exported = remainingNode.filter((entry) => entry.id.startsWith('export'));
  const endings = remainingNode.filter((entry) => ['archive', 'unarchive', 'delete'].includes(entry.id));
  const organized = remainingNode.filter((entry) => !entry.id.startsWith('export') && !['archive', 'unarchive', 'delete'].includes(entry.id) && entry.type !== 'separator');
  const entries: MenuEntry[] = [
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
        { id: 'find', label: t('search.find.command'), icon: <Search aria-hidden />, onSelect: () => openFind(thread.id, instance?.id) },
        { id: 'notifications', label: t('thread.bell.label'), onSelect: () => setNotificationsOpen(true) },
        { id: 'running', label: t('running.title'), onSelect: () => setRunningOpen(true) },
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
    <><StatusPlace place="header" threadId={thread.id} className="shrink-0"><header className="flex shrink-0 flex-col gap-1 border-b border-border px-4 py-2" aria-label={t('thread.header.label')} data-thread-header>
      <div className="flex min-w-0 flex-wrap items-center gap-2" data-thread-title-row>
        <div className={cx('flex min-w-0 items-start gap-2', compact ? 'basis-full' : 'basis-0 flex-1')}><div className="min-w-0 flex-1"><Title thread={thread} mayEdit={mayManage && !thread.archived} /></div><StateFact thread={thread} compact /><Hint id="thread-states" popover /></div>
        <div className={cx('flex shrink-0 flex-wrap items-center gap-1', compact ? 'w-full' : 'w-auto')} data-thread-actions-row>
          <div role="group" aria-label={t('thread.header.quickActions')} className="flex shrink-0 items-center gap-1">
            {headerActions.map((entry) => <IconButton key={entry.id} size="sm" label={choiceName(entry)} icon={entry.icon} blocked={Boolean(entry.disabled)}
              reason={typeof entry.disabled === 'string' ? entry.disabled : undefined} onClick={entry.onSelect} />)}
            <IconButton size="sm" icon={<Search aria-hidden />} label={t('search.find.command')} onClick={() => openFind(thread.id, instance?.id)} />
          </div>
          <span className="min-w-0 flex-1" />
          <StarButton place={place} />
          <SubscriptionBell thread={thread} />
          <IconButton size="sm" action="share" label={actionName('share')} onClick={() => openAccess({ kind: 'thread', id: thread.id, name: thread.name })} />
          <Menu label={actionName('actions', thread.name)} align="end" entries={entries} trigger={<IconButton size="sm" label={actionName('actions', thread.name)} icon={<Ellipsis aria-hidden />} />} />
        </div>
      </div>
      {importedHistory && <StateLine compact icon={<History aria-hidden />} message={t('thread.header.importedHistory')}
        action={thread.binding === null && !thread.archived && thread.side_of === null ? <Tooltip content={setupReason}>
          <Button size="sm" blocked={setupReason !== null} onClick={() => setSettingUp(thread.id)}>{t('thread.header.chooseAgentProvider')}</Button>
        </Tooltip> : undefined} />}
      <AdminReachNotice of={thread.access.admin_access?.place} private={thread.page === 'restricted'} object={{ kind: 'thread', id: thread.id }} />
      {thread.description.trim() !== '' && <DescriptionLine thread={thread} open={describing} onToggle={() => { setEditingDescription(false); setDescribing(true); }} mayEdit={false} />}
      <SubagentChips threadId={thread.id} onReveal={onRevealItem} />
      {describing && <Dialog open onOpenChange={setDescribing} title={t('thread.description.field')}><DescriptionPanel key={`${thread.description}:${editingDescription}`} thread={thread} initialEdit={editingDescription} mayEdit={mayManage && !thread.archived} onClose={() => setDescribing(false)} /></Dialog>}
      {tagging && <Dialog open onOpenChange={setTagging} title={t('placeTags.title')}><TagPicker place={thread} allowed={holds(thread.access, 'thread.interact') && !thread.archived} /></Dialog>}
      {catchupOpen && <CatchupRequest threadId={thread.id} open onOpenChange={setCatchupOpen} />}
    </header></StatusPlace>
    {settingUp === thread.id && thread.binding === null && !thread.archived && thread.side_of === null && setupReason === null && <SetUpThread thread={thread} onClose={() => setSettingUp(null)} />}
    {notificationsOpen && <Dialog open onOpenChange={setNotificationsOpen} title={t('thread.bell.label')}><SubscriptionBell thread={thread} /></Dialog>}
    {runningOpen && <RunningDialog thread={thread.id} close={() => setRunningOpen(false)} />}
    {sidesOpen && <Dialog open onOpenChange={setSidesOpen} title={t('side.chip.label')}><SideThreadList thread={thread} onOpened={() => setSidesOpen(false)} onRevealItem={(id) => onRevealItem?.(id)} /></Dialog>}
    {peopleOpen && <Dialog open onOpenChange={setPeopleOpen} title={t('composer.row.looking')}><PresenceStack users={presence} without={meId} /><ul className="flex flex-col gap-2">{presence.filter((entry) => entry.user.id !== meId).map((entry) => <li key={entry.user.id}><UserChip user={entry.user} /></li>)}</ul></Dialog>}
    {movingTopic && <MoveToTopic key={thread.id} thread={thread} close={() => setMovingTopic(false)} />}
    </>
  );
}
