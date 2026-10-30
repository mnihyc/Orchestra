/** Several destinations of four kinds, a comment, a preview and the stated effect; what the agent said and did in one turn arrives as one part.
 *
 * "Forward…" opens this one dialog. To: one field that takes several places of four kinds, each with
 * its icon: the chats of places, the person's groups, people (the direct chat with them in this org),
 * and agents' threads, with the state each is in; at most ten. Add a comment, if wanted, sent above what
 * is forwarded. A preview of what will arrive, and one sentence on what the choice means: "2 chats and
 * Bo will get a copy of these 3 messages", and, for a thread, that its agent will read them as a message
 * from the sender, which counts as theirs. The button repeats it: "Forward to 3 places". If any one
 * place refuses, nothing is sent and the dialog says which and why. The items of a thread's conversation
 * go anywhere a message does; into that thread's own chat, offered first, they go as live quotes.
 */
import { ThreadStateIcon } from '@/ui/ThreadState';
import { useInfiniteQuery, useQueries, useQuery } from '@tanstack/react-query';
import { Bot, Diamond, Folder, Info, LayoutGrid, MessageSquareText, Search, User, Users, X } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { ThreadIdentity } from '@/features/people/ThreadIdentity';
import { useStore } from 'zustand';

import { ApiError, client } from '@/api/client';
import { nodeQuery } from '@/api/nodes';
import { holds } from '@/lib/permissions';
import { mentionNames } from '@/features/people/wording';
import { ErrorState } from '@/ui/ErrorState';
import { forward, orgChatsQuery, peopleQuery, PEOPLE_SEARCH_MIN_CHARS } from '@/api/chats';
import { firstLine, plain, textOfParts } from '@/features/chat/text';
import {
  type Destination,
  destinationsOf,
  groupDestinations,
  MOST_DESTINATIONS,
  nodeDestinations,
  personDestinations,
  refusedDestination,
  summary,
  threadDestinations,
} from '@/features/forward/destinations';
import type { ChatMessage, ForwardCreateReply, NodeKind } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useActKey } from '@/lib/forms';
import { useDebounced } from '@/lib/debounce';
import { pathTo, useOrgTree } from '@/realtime/stores/tree';
import { Avatar } from '@/ui/Avatar';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { useErrorText } from '@/ui/errorText';
import { InlineAlert } from '@/ui/InlineAlert';
import { fieldBoxClasses } from '@/ui/Input';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

/** What is forwarded: messages of a chat, or items of a thread. */
export type ForwardSource =
  | {
      kind: 'chat';
      chatId: string;
      /** The root org the chat belongs to: people are the direct chats there. */
      orgId: string;
      /** What the source is called, for the line above the preview. */
      label: string;
      messages: readonly ChatMessage[];
    }
  | {
      kind: 'thread';
      threadId: string;
      orgId: string;
      label: string;
      /** The chat of the thread, offered first: there its items stay live quotes, elsewhere they are copies. */
      threadChat: { chatId: string; label: string };
      itemIds: readonly string[];
      /** What the items say, in their order, for the preview. */
      preview: readonly { id: string; author: string; at: string; text: string; agent?: boolean; avatar?: { text: string; color: number } }[];
    };

const NODE_ICONS: Record<NodeKind, typeof Diamond> = { org: LayoutGrid, project: Folder, topic: Diamond, thread: MessageSquareText };

/** How many places of each kind the list shows before the person narrows it by typing. */
const SHOWN_PER_KIND = 6;
/** How many characters of a message the preview shows. */
const PREVIEW_CHARS = 140;

function DestinationIcon({ destination }: { destination: Destination }) {
  if (destination.kind === 'thread' || destination.kind === 'node' && destination.nodeKind === 'thread') return <ThreadStateIcon display={destination.display} className="[&_svg]:size-4" />;
  const Icon =
    destination.kind === 'node' ? NODE_ICONS[destination.nodeKind] : destination.kind === 'group' ? Users : User;
  return <Icon size={16} aria-hidden className="shrink-0 text-text-muted" />;
}

export interface ForwardDialogProps {
  initialDestinations?: readonly Destination[];
  source: ForwardSource | null;
  onClose: () => void;
  /** Told what was written, when the forward went through. */
  onForwarded?: (reply: ForwardCreateReply) => void;
}

export function ForwardDialog({ source, onClose, onForwarded, initialDestinations }: ForwardDialogProps) {
  return source === null ? null : <ForwardForm source={source} initialDestinations={initialDestinations ?? []} onClose={onClose} {...(onForwarded === undefined ? {} : { onForwarded })} />;
}

function ForwardForm({ source, onClose, onForwarded, initialDestinations }: { source: ForwardSource; initialDestinations: readonly Destination[]; onClose: () => void; onForwarded?: (reply: ForwardCreateReply) => void }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const locale = useAppearance((state) => state.locale);
  const me = useStore(client.session, (state) => state.userId);
  const actKey = useActKey();
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState<Destination[]>(() => [...initialDestinations]);
  const threadChecks = useQueries({ queries: chosen.flatMap((target) => target.kind === 'thread' ? [{ ...nodeQuery(target.threadId), retry:false, staleTime:0 }] : []) });
  const threadInteract = 'thread.interact';
  const mayAsk = threadChecks.every((read) => read.isSuccess && !read.isFetching && read.data.kind === 'thread' && holds(read.data.access, threadInteract) && !read.data.archived);
  const [comment, setComment] = useState('');
  const [pending, setPending] = useState(false);
  const [refusal, setRefusal] = useState<{ error: unknown; destination: Destination | null } | null>(null);
  const settled = useDebounced(query);

  const tree = useOrgTree(source.orgId);
  const groups = useInfiniteQuery(orgChatsQuery(source.orgId, { scope: 'mine', of: null }));
  const found = useQuery({ ...peopleQuery(source.orgId, settled.trim()), enabled: Array.from(settled.trim()).length >= PEOPLE_SEARCH_MIN_CHARS });

  const choices = useMemo<Destination[]>(() => {
    const pathOf = (node: { id: string }): string | null => {
      const above = pathTo(tree, node.id).slice(0, -1);
      return above.length === 0 ? null : above.map((entry) => entry.name).join(' / ');
    };
    // The items of a thread go first to its own chat, where they stay live quotes.
    const home: Destination[] =
      source.kind === 'thread'
        ? [{ key: `chat:${source.threadChat.chatId}`, kind: 'group', chatId: source.threadChat.chatId, label: source.threadChat.label, detail: null }]
        : [];
    const nodes = nodeDestinations(tree.nodes.values(), query, pathOf).filter((one) => !home.some((first) => first.key === one.key));
    const mine = groupDestinations(groups.data?.pages.flatMap((page) => page.items) ?? [], query, me);
    const people = personDestinations(found.data ?? [], me);
    const agents = threadDestinations(tree.nodes.values(), query, pathOf, source.kind === 'thread' ? source.threadId : null);
    const capped = query.trim() === '' ? SHOWN_PER_KIND : SHOWN_PER_KIND * 2;
    return [...home, ...mine.slice(0, capped), ...people.slice(0, capped), ...agents.slice(0, capped), ...nodes.slice(0, capped)];
  }, [source, tree, query, groups.data, found.data, me]);

  const total = source.kind === 'chat' ? source.messages.length : source.itemIds.length;
  const sums = summary(chosen);
  const toggle = (destination: Destination) => {
    setRefusal(null);
    setChosen((before) =>
      before.some((one) => one.key === destination.key)
        ? before.filter((one) => one.key !== destination.key)
        : before.length >= MOST_DESTINATIONS
          ? before
          : [...before, destination],
    );
  };

  const sentence = (() => {
    if (chosen.length === 0) return null;
    const list = new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' });
    const who = [
      ...(sums.chats > 0 ? [t('forward.effect.chats', { count: sums.chats })] : []),
      ...(sums.people.length > 2 ? [t('forward.effect.people', { count: sums.people.length })] : sums.people),
    ];
    const copies = who.length === 0 ? null : t('forward.effect.will', { who: list.format(who), count: total });
    // What goes to a thread is read by its agent, as the sender's own message, and its files and images are placed in
    // the thread's workspace.
    const agents = sums.threads.length === 0 ? null : t('forward.effect.agents', { count: sums.threads.length, thread: sums.threads[0] ?? '' });
    const carried = source.kind === 'chat' ? source.messages.flatMap((message) => message.parts) : [];
    const files = carried.filter((part) => part.type === 'attachment').length;
    const images = carried.filter((part) => part.type === 'image').length;
    const what = [
      ...(files > 0 ? [t('forward.effect.files', { count: files })] : []),
      ...(images > 0 ? [t('forward.effect.images', { count: images })] : []),
    ];
    const placed = sums.threads.length === 0 || what.length === 0 ? null : t('forward.effect.placed', { count: sums.threads.length, what: list.format(what) });
    return [copies, agents, placed].filter((part) => part !== null).join(' ');
  })();

  const submit = async () => {
    if (!mayAsk) return;
    if (chosen.length === 0 || pending) return;
    setPending(true);
    setRefusal(null);
    const body = {
      from: source.kind === 'chat' ? { chat_id: source.chatId, message_ids: source.messages.map((message) => message.id) } : { thread_id: source.threadId, item_ids: [...source.itemIds] },
      to: destinationsOf(chosen),
      client_id: actKey({ source: source.kind === 'chat' ? source.messages.map((m) => m.id) : source.itemIds, to: chosen.map((one) => one.key), comment }),
      ...(comment.trim() === '' ? {} : { comment: comment.trim() }),
    };
    try {
      const reply = await forward(body);
      toast({ tone: 'success', title: t('forward.done', { count: chosen.length }) });
      onForwarded?.(reply);
      onClose();
    } catch (error) {
      setRefusal({ error, destination: error instanceof ApiError ? refusedDestination(chosen, error.problem.object) : null });
      setPending(false);
    }
  };

  const preview =
    source.kind === 'chat'
      ? source.messages.map((message) => ({
          id: message.id,
          author: message.author !== null && 'display_name' in message.author ? message.author.display_name : (message.author?.agent ?? ''),
          // The avatar of a person is the one the chat shows beside what they wrote.
          avatar: message.author !== null && 'avatar' in message.author ? message.author.avatar : undefined,
          at: message.created_at,
          agent: message.author !== null && 'agent' in message.author,
          text: message.deleted ? t('chat.deleted') : mentionNames(firstLine(message.parts, PREVIEW_CHARS), message.mentions),
          wholeText: message.deleted ? t('chat.deleted') : mentionNames(plain(textOfParts(message.parts)), message.mentions),
        }))
      : source.preview.map((entry) => ({ ...entry, wholeText: entry.text, text: firstLine([{ type: 'text', text: entry.text }], PREVIEW_CHARS) }));

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !pending && onClose()}
      title={t('forward.title')}
      description={t('forward.description', { count: total })}
      size="lg"
      dismissible={!pending}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={pending}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={pending} blocked={chosen.length === 0 || !mayAsk} onClick={() => void submit()}>
            {chosen.length === 0 ? t('forward.buttonNone') : t('forward.button', { count: chosen.length })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <section aria-label={t('forward.to')} className="flex flex-col gap-2">
          <h3 className="text-sm font-medium text-text">{t('forward.to')}</h3>
          {chosen.length > 0 && (
            <ul className="flex flex-wrap gap-1" aria-label={t('forward.chosen')}>
              {chosen.map((one) => (
                <li key={one.key} className="inline-flex max-w-full items-center gap-1 rounded-full border border-border bg-bg-subtle py-1 pr-1 pl-2 text-sm">
                  <DestinationIcon destination={one} />
                  <span className="truncate">{one.label}</span>
                  <button
                    type="button"
                    aria-label={t('forward.remove', { name: one.label })}
                    onClick={() => toggle(one)}
                    className="inline-flex size-5 items-center justify-center rounded-full text-text-muted hover:bg-bg-muted hover:text-text"
                  >
                    <X size={16} aria-hidden />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <label className="relative block">
            <span className="sr-only">{t('forward.search')}</span>
            <Search aria-hidden size={16} className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-text-subtle" />
            <input
              type="search"
              value={query}
              autoComplete="off"
              placeholder={t('forward.search')}
              onChange={(event) => setQuery(event.target.value)}
              className={`${fieldBoxClasses} input-leading-icon`}
            />
          </label>
          <div role="group" aria-label={t('forward.found')} className="flex max-h-[30dvh] flex-col gap-1 overflow-y-auto rounded-lg border border-border p-1">
            {choices.length === 0 ? (
              <EmptyState compact className="px-2" title={t('forward.nothing')} />
            ) : (
              choices.map((one) => {
                const on = chosen.some((held) => held.key === one.key);
                return (
                  <button
                    key={one.key}
                    type="button"
                    aria-pressed={on}
                    onClick={() => toggle(one)}
                    className={cx('focus-inset flex items-center gap-2 rounded-md px-2 py-1 text-left text-base hover:bg-bg-muted', on && 'bg-accent-subtle')}
                  >
                    <DestinationIcon destination={one} />
                    <span className="truncate font-medium">{one.label}</span>
                    {one.detail !== null && <span className="truncate text-sm text-text-muted">{one.detail}</span>}
                    {one.kind === 'thread' && <ThreadIdentity id={one.threadId} />}
                    <span className="ml-auto shrink-0 text-xs text-text-muted">{t(`forward.kind.${one.kind}`)}</span>
                  </button>
                );
              })
            )}
          </div>
          {chosen.length >= MOST_DESTINATIONS && <p className="text-xs text-text-muted">{t('forward.most', { count: MOST_DESTINATIONS })}</p>}
          {source.kind === 'thread' && (
            <p className="flex items-start gap-1 text-xs text-text-muted">
              <Info size={16} aria-hidden className="mt-0.5 shrink-0" />
              <span>{t('forward.ownChat')}</span>
            </p>
          )}
        </section>

        <Textarea
          label={t('forward.comment')}
          optional
          rows={2}
          value={comment}
          disabled={pending}
          onChange={(event) => setComment(event.target.value)}
        />

        <section aria-label={t('forward.preview')} className="flex flex-col gap-1">
          <h3 className="text-sm font-medium text-text">{t('forward.preview')}</h3>
          <div className="flex max-h-[25dvh] flex-col gap-2 overflow-y-auto rounded-lg border border-border bg-bg-subtle p-3" tabIndex={0}>
            <p className="text-xs text-text-muted">{t('chat.forwarded.fromLabel', { name: source.label })}</p>
            {preview.map((entry) => (
              <div key={entry.id} className="flex items-start gap-2 text-sm">
                {entry.agent === true ? <Bot aria-hidden className="size-4 shrink-0 text-text-muted" /> : <Avatar text={entry.avatar?.text ?? Array.from(entry.author)[0]?.toUpperCase() ?? '?'} color={entry.avatar?.color ?? 0} size={24} />}
                <div className="min-w-0">
                  <span className="font-medium text-text">{entry.author}</span>
                  <p data-overflow-text={entry.wholeText} className="truncate text-text-muted">{entry.text}</p>
                </div>
              </div>
            ))}
          </div>
        </section>

        {sentence !== null && <p className="text-sm text-text">{sentence}</p>}
        {threadChecks.map((read, index) => <div key={index} className="flex flex-col gap-1 text-sm text-text-muted">
          {read.isPending ? <p role="status">{t('common.loading')}</p> : read.isError ? <ErrorState compact error={read.error} /> : read.data.kind === 'thread' ? <>
            <p>{t('people.forwardModel', { model:read.data.model ?? t('people.forwardNoModel') })}</p>
            <p>{t('people.forwardQueuedUse', { thread:read.data.name })}</p>
            {!holds(read.data.access,threadInteract) && <p>{t('forward.refused')}</p>}
          </> : <p>{t('forward.refused')}</p>}
        </div>)}
        {refusal !== null && (
          <InlineAlert tone="danger" title={t('forward.refused')}>
            {refusal.destination !== null && <span className="font-medium">{refusal.destination.label}: </span>}
            {describe(refusal.error).message}
          </InlineAlert>
        )}
      </div>
    </Dialog>
  );
}
