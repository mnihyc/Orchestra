/** Common agent-turn actions share the existing input, fork, side and context flows. */
import type { ReactNode } from 'react';
import { linkTo, referenceTo } from '@/lib/links';
import { ContextMenu } from '@/ui/ContextMenu';
import { useMutation } from '@tanstack/react-query';
import { Copy, Ellipsis, GitFork, Link, ListChecks, MessageSquareDashed, ScrollText } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { retryTurn, sendInput } from '@/api/threads';
import { openChange } from '@/features/change/target';
import { openGiven } from '@/features/inspector/givenFocus';
import { sideReadingAvailable, sideRefusal } from '@/features/side-threads/availability';
import { openSideAsk } from '@/features/side-threads/ask';
import { threadCapabilities } from '@/features/thread/cached';
import { useImmediateFork } from '@/features/thread/ImmediateFork';
import { useTimelineContext } from '@/features/timeline/context';
import { HoverActions } from '@/features/timeline/HoverActions';
import { selectionMarkdown } from '@/features/timeline/SelectionBar';
import type { Turn } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { turnPasteSource } from '@/lib/turnPasteSource';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadViewOf, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { IconButton } from '@/ui/IconButton';
import { ErrorAlert } from '@/ui/ErrorState';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { copyText } from '@/ui/clipboard';

function ImmediateForkAction({ turn, blocked, fork }: { turn: Turn; blocked: string | null; fork: ReturnType<typeof useImmediateFork> }) {
  const { t } = useTranslation();
  return <><IconButton size="sm" icon={<GitFork aria-hidden />} label={t('timeline.selection.forkNow')}
    blocked={blocked !== null || fork.pending} reason={blocked ?? (fork.pending ? t('change.forkNow.making') : undefined)} onClick={() => fork.start(turn.ordinal)} /></>;
}

export function TurnActions({ turn, ids, records, inputItemId, children }: { turn: Turn; ids: readonly string[]; records: readonly string[]; inputItemId?: string; children?: (actions: ReactNode) => ReactNode }) {
  const { t } = useTranslation();
  const context = useTimelineContext();
  const read = useThreadReadRef(context.threadId);
  const thread = useThreadPart(context.threadId, (held) => held.thread);
  const fork = useImmediateFork(thread);
  const turns = useThreadPart(context.threadId, (held) => held.turns);
  const last = ![...turns.values()].some((candidate) => candidate.ordinal > turn.ordinal);
  const stopped = turn.state === 'failed' || turn.state === 'interrupted';
  const original = useThreadPart(context.threadId, (held) => turnPasteSource(held.inputs, held.items, turn.input_id, inputItemId));
  const items = useThreadPart(context.threadId, useShallow((held) => ids.flatMap((id) => held.items.get(id) ?? [])));
  const reason = context.readOnly === true ? t('messageVersions.activeActions') : thread?.archived === true ? t('thread.archived.lineAnonymous') : !context.canInteract ? t('composer.row.readOnly')
    : !context.live ? t('approvals.waitForLine') : !stopped && !last ? t('thread.turnActions.retryMiddle')
    : stopped && thread?.blocked?.detail === 'call_recovery' ? t('thread.turnActions.retryHandled') : !stopped && (turn.state !== 'completed' || original === undefined || original.length === 0) ? t('thread.turnActions.noInput') : null;
  const retry = useMutation({
    mutationFn: async () => {
      // Re-select admitted input at the press; this is an ordinary repeated input, never a native command replay.
      const held = threadViewOf(read);
      const content = turnPasteSource(held.inputs, held.items, turn.input_id, inputItemId);
      if (stopped) return retryTurn(read, turn.id, newActKey());
      if ([...held.turns.values()].some((one) => one.ordinal > turn.ordinal) || content === undefined || content.length === 0) throw new Error(t('thread.turnActions.noInput'));
      return sendInput(read, { content: [...content], delivery: 'queue', clientId: newActKey() });
    },
    onSuccess: (reply) => threadWriter.apply(read, { inputs: [reply.input] }),
  });
  const forkReason = context.readOnly === true ? t('messageVersions.activeActions') : threadCapabilities(thread)?.fork !== true || (thread?.side_of !== null && thread?.side_of !== undefined)
    ? t('thread.turnActions.forkUnavailable') : thread?.archived === true ? t('thread.archived.lineAnonymous') : null;
  const sideReason = sideRefusal(thread, t);
  const anchor = [...items].reverse().find((item) => item.kind === 'agent_message') ?? items.at(-1);
  const continuation = useThreadPart(context.threadId, (held) => held.read?.projection?.viewed_continuation_id);
  const origin = context.inspection?.threadId ?? context.threadId;
  const entries: MenuEntry[] = [
    { id: 'fork-to', label: t('timeline.selection.forkTo'), icon: <GitFork aria-hidden />, disabled: forkReason ?? false,
      onSelect: () => thread?.program?.id === 'orch' ? fork.start(turn.ordinal)
        : openChange({ kind: 'thread', id: context.threadId, name: context.threadName }, { kind: 'fork', toTurn: turn.ordinal }) },
    ...(thread !== null && thread.side_of === null && sideReadingAvailable(thread) ? [{ id: 'side', label: t('side.item.ask'), icon: <MessageSquareDashed aria-hidden />,
      disabled: context.readOnly === true ? t('messageVersions.activeActions') : !context.canInteract ? t('composer.row.readOnly') : sideReason,
      onSelect: () => openSideAsk({ threadId: context.threadId, threadName: context.threadName, toTurn: turn.ordinal, text: '', read }) } as MenuEntry] : []),
    { id: 'select-turn', label: t('timeline.selection.turn'), icon: <ListChecks aria-hidden />, disabled: context.readOnly === true ? t('messageVersions.activeActions') : false, onSelect: () => context.selection.pick(ids) },
    ...(records.length === 0 ? [] : [{ id: 'context', label: t('threadViews.given.context'), icon: <ScrollText aria-hidden />,
      onSelect: () => records.length === 1 && records[0] !== undefined ? openGiven(context.threadId, { record: records[0] }) : openGiven(context.threadId, { turn: turn.id }) } as MenuEntry]),
    { id: 'copy-link', label: t('timeline.actions.copyLink'), icon: <Link aria-hidden />, disabled: anchor === undefined ? t('thread.turnActions.noItems') : false,
      onSelect: () => { if (anchor !== undefined) void copyText(linkTo({ kind: 'thread', id: origin }, { turn: turn.ordinal, item: anchor.id, ...(continuation === undefined ? {} : { continuation }) }), t('common.linkCopied')); } },
    { id: 'copy-reference', label: t('people.reference.copy'), icon: <Link aria-hidden />,
      onSelect: () => void copyText(referenceTo({ kind: 'thread', id: origin, name: t('people.reference.turn', { number: turn.ordinal }) }, { turn: turn.ordinal }), t('people.reference.copied')) },
  ];
  const interactive = context.inspection === undefined;
  const menu = interactive ? entries : entries.filter((entry) => 'id' in entry && (entry.id === 'copy-link' || entry.id === 'copy-reference'));
  const actions = <><HoverActions owner="turn">
    <IconButton size="sm" icon={<Copy aria-hidden />} label={t('thread.turnActions.copy')} reason={t(items.length === 0 ? 'thread.turnActions.noItems' : 'thread.turnActions.copyAbout')} blocked={items.length === 0}
      onClick={() => void copyText(selectionMarkdown(items), t('thread.turnActions.copied'))} />
    {interactive && <><IconButton size="sm" action="retryTurn" label={t('timeline.footer.retry')} blocked={reason !== null} reason={reason ?? undefined} pending={retry.isPending} onClick={() => retry.mutate()} />
    {thread !== null && threadCapabilities(thread)?.fork === true ? <ImmediateForkAction turn={turn} blocked={forkReason} fork={fork} />
      : <IconButton size="sm" icon={<GitFork aria-hidden />} label={t('timeline.selection.forkNow')} blocked reason={forkReason ?? t('thread.turnActions.forkUnavailable')} />}</>}
    <Menu label={t('thread.turnActions.more')} align="end" entries={menu} trigger={<IconButton size="sm" label={t('thread.turnActions.more')} icon={<Ellipsis aria-hidden />} />} />
  </HoverActions>{fork.confirmation}{interactive && retry.error !== null && <ErrorAlert error={retry.error} />}</>;
  return <ContextMenu label={t('thread.turnActions.more')} entries={menu}><div className="contents">{children === undefined ? actions : children(actions)}</div></ContextMenu>;
}
