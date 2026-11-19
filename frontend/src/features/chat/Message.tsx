/** Grouped message with replies, mentions, edits and deletions.
 *
 * One message of a chat: its author's header when it begins a run, the line it answers, its text and
 * whatever else it carries, "(edited)" or "Message deleted" where that happened, the reactions under
 * it, and its toolbar and menu. The same entries are in the toolbar's "more", in the menu under the
 * right mouse button and on the keyboard's menu key. A group's events, a message being sent and one
 * that was not sent are drawn here too.
 */
import { CornerUpLeft, EllipsisVertical, Pin, Reply } from 'lucide-react';
import { memo, type ReactNode, useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';

import { actionName } from '@/ui/actions';
import { AgentAvatar, AgentName } from '@/features/chat/AgentMessage';
import { discard, edit, resend, useBusy } from '@/features/chat/acts';
import { FileChip, ImageThumb } from '@/features/chat/Attachments';
import { AttachmentChips } from '@/features/composer/Attachments';
import { readPastedText } from '@/lib/pastedText';
import { Body } from '@/features/chat/Body';
import { type MessageActions, useChatContext } from '@/features/chat/context';
import { QuotedItem } from '@/features/chat/QuotedItem';
import { Reactions } from '@/features/chat/Reactions';
import { isMine } from '@/features/chat/rows';
import { editedParts, firstLine, textOfParts } from '@/features/chat/text';
import { draftMentionText, editedMentionNames, typedTextParts } from '@/features/people/mentionParts';
import { mentionNames } from '@/features/people/wording';
import { AskedAgents } from '@/features/people/AgentAsks';
import { PersonThingProvider } from '@/features/people/personContext';
import { ForwardedBlocks } from '@/features/forward/ForwardedBlock';
import type { Chat, ChatMessage, UserRef } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { isForwarded } from '@/lib/content';
import { isComposing } from '@/lib/shortcuts';
import type { Outgoing } from '@/realtime/stores/chat';
import { UserChip } from '@/ui/UserChip';
import { Button } from '@/ui/Button';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { ReactButton } from '@/ui/EmojiPicker';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import { Textarea } from '@/ui/Textarea';

/**
 * The hour and minute alone, for the gutter beside a message that joins the one above it: the day is on its
 * divider, and the exact moment is on the header of the run. Without "AM" and "PM", so that it fits a gutter of one avatar.
 */
function gutterTime(moment: string, locale: string, timeZone: string): string {
  return new Intl.DateTimeFormat(locale, { timeZone, hour: '2-digit', minute: '2-digit' })
    .formatToParts(new Date(moment))
    .filter((part) => part.type !== 'dayPeriod')
    .map((part) => part.value)
    .join('')
    .trim();
}

/** How many characters of the message it answers a reply shows above it. */
const REPLY_EXCERPT_CHARS = 120;

/** What the reader may do with a message, from the chat's `can` and who wrote it. */
export function messagePowers(chat: Chat, message: ChatMessage, me: string | null) {
  const mine = isMine(message, me);
  const hasText = message.parts.some((part) => !isForwarded(part) && part.type === 'text');
  return {
    react: chat.can.send && !message.deleted,
    reply: chat.can.send && !message.deleted,
    edit: mine && chat.can.send && !message.deleted && (hasText || message.parts.length === 0),
    remove: !message.deleted && ((mine && chat.can.send) || chat.can.moderate),
    pin: chat.can.send && !message.deleted,
    // A deleted message has nothing left to forward, copy or link to.
    forward: !message.deleted,
    copy: !message.deleted && hasText,
  };
}

/** The entries of a message's menu, in the order the menu keeps: what a click would do first, what destroys last. */
export function messageEntries(
  t: (key: string) => string,
  chat: Chat,
  message: ChatMessage,
  me: string | null,
  acts: MessageActions,
): MenuEntry[] {
  const power = messagePowers(chat, message, me);
  const entries: MenuEntry[] = [];
  if (power.reply) entries.push({ id: 'reply', label: t('chat.actions.reply'), icon: <Reply aria-hidden />, onSelect: () => acts.reply(message) });
  if (power.forward) entries.push({ id: 'forward', label: t('chat.actions.forward'), onSelect: () => acts.forward(message) });
  if (!message.deleted && chat.can.reason !== 'inspecting' && acts.unread !== undefined) entries.push({ id: 'unread', label: t('people.markUnread'), onSelect: () => acts.unread?.(message) });
  entries.push({ id: 'select', label: t('chat.actions.select'), onSelect: () => acts.select(message) });
  if (power.edit) entries.push({ id: 'edit', label: t('chat.actions.edit'), onSelect: () => acts.edit(message) });
  if (power.pin) {
    const on = message.pinned === null;
    entries.push({ id: 'pin', label: t(on ? 'chat.actions.pin' : 'chat.actions.unpin'), icon: <Pin aria-hidden />, onSelect: () => acts.pin(message, on) });
  }
  if (power.copy) entries.push({ id: 'copy-text', label: t('chat.actions.copyText'), onSelect: () => acts.copyText(message) });
  entries.push({ id: 'copy-link', label: t('chat.actions.copyLink'), onSelect: () => acts.copyLink(message) });
  if (power.remove) {
    entries.push({ type: 'separator', id: 'before-delete' });
    entries.push({ id: 'delete', label: t('chat.actions.delete'), danger: true, onSelect: () => acts.remove(message) });
  }
  return entries;
}

// ---------------------------------------------------------------- the events of a group

function SystemLine({ message }: { message: ChatMessage }) {
  const { t } = useTranslation();
  const system = message.system;
  if (system === null) return null;
  if(system.event==='member_expired')return <li className="flex items-center justify-center gap-1 px-4 py-1 text-sm text-text-muted">{system.user!==null&&<UserChip user={system.user} size={24}/>}<span>{t('chat.system.membershipEnded')}</span></li>;
  const key = ({
    created: 'chat.system.createdChip',
    renamed: system.name === null ? 'chat.system.unnamedChip' : 'chat.system.renamedChip',
    member_added: 'chat.system.addedChip',
    member_left: 'chat.system.leftChip',
    member_removed: 'chat.system.removedChip',
  } as const)[system.event];
  return (
    <li className="flex items-center justify-center px-4 py-1 text-center text-xs text-text-muted">
      <span className="wrap-anywhere"><Trans i18nKey={key} values={{ name: system.name ?? '' }} components={{
        by: system.by === null ? <span /> : <UserChip user={system.by} size={24} />,
        person: system.user === null ? <span /> : <UserChip user={system.user} size={24} />,
      }} /></span>
    </li>
  );
}

// ---------------------------------------------------------------- what a message carries

function Parts({ message }: { message: ChatMessage }): ReactNode {
  const { chat, me } = useChatContext();
  const forwarded = message.parts.filter(isForwarded);
  return (
    <>
      {message.parts.map((part, index) => {
        if (isForwarded(part)) return null;
        switch (part.type) {
          case 'text':
            return <Body key={index} text={part.text} mentions={message.mentions} mentionsAll={message.mentions_all} me={me} originThread={message.author !== null && 'agent' in message.author && !message.author.thread.redacted ? message.author.thread.id : undefined} />;
          case 'pasted_text':
            return <AttachmentChips key={index} parts={[part]} readPaste={(paste) => readPastedText('chat', chat.id, paste)} />;
          case 'image':
            return <ImageThumb key={index} chatId={chat.id} blob={part.blob} name={part.name} mediaType={part.media_type} owner={message.author ?? undefined} updatedAt={message.created_at} />;
          case 'attachment':
            return <FileChip key={index} chatId={chat.id} blob={part.blob} name={part.name} size={part.size} mediaType={part.media_type} owner={message.author ?? undefined} updatedAt={message.created_at} />;
          case 'item_ref':
            return <QuotedItem key={index} threadId={part.thread_id} itemId={part.item_id} />;
        }
      })}
      {forwarded.length > 0 && <ForwardedBlocks parts={forwarded} chatId={chat.id} />}
    </>
  );
}

/** The editor of a message in its place: Enter saves, Shift+Enter is a new line, Escape puts it back. */
function MessageEditor({ message }: { message: ChatMessage }) {
  const { t } = useTranslation();
  const { chat, setEditing } = useChatContext();
  const [text, setText] = useState(() => textOfParts(message.parts));
  const [names, setNames] = useState(message.mention_names ?? []);
  const [saving, setSaving] = useState(false);
  const editingParts = () => [...typedTextParts(text, names), ...editedParts(message.parts, '')];
  const save = async () => {
    if (saving) return;
    const parts = editingParts();
    if (parts.length === 0) return;
    setSaving(true);
    const done = await edit(chat.id, message.id, parts);
    setSaving(false);
    if (done) setEditing(null);
  };
  return (
    <div className="flex flex-col gap-2">
      <Textarea
        label={t('chat.edit.label')}
        hideLabel
        autoFocus
        rows={2}
        value={text}
        disabled={saving}
        onChange={(event) => { setNames(editedMentionNames(text, event.target.value, names)); setText(event.target.value); }}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            setEditing(null);
          } else if (event.key === 'Enter' && !event.shiftKey && !isComposing(event.nativeEvent)) {
            event.preventDefault();
            void save();
          }
        }}
      />
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" variant="primary" pending={saving} disabled={editingParts().length === 0} onClick={() => void save()}>
          {t('chat.edit.save')}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setEditing(null)}>
          {t('common.cancel')}
        </Button>
        <span className="text-xs text-text-muted">{t('chat.edit.hint')}</span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- one message

export interface MessageProps {
  message: ChatMessage;
  /** Whether it begins a run, with its author's header. */
  head: boolean;
}

export const Message = memo(function Message({ message, head }: MessageProps) {
  const { t } = useTranslation();
  const { chat, me, people, acts, editing, selection, lit } = useChatContext();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const pending = useBusy((state) => state.ids.has(message.id));
  if (message.kind === 'system') return <PersonThingProvider thing={{ kind: 'chat', id: chat.id, anchor: { message: message.id } }}><SystemLine message={message} /></PersonThingProvider>;

  const author = message.author;
  const power = messagePowers(chat, message, me);
  const entries = messageEntries(t as (key: string) => string, chat, message, me, acts);
  const isEditing = editing === message.id;
  const picked = selection.has(message.id);
  const agent = author !== null && 'agent' in author ? author : null;
  const person = author !== null && 'id' in author ? author : null;
  const replied = message.reply_to;
  const menuName = actionName('actions', `${person?.display_name ?? agent?.agent ?? t('craftActions.message')} · ${gutterTime(message.created_at, locale, timeZone)}`);

  return (
    <PersonThingProvider thing={{ kind: 'chat', id: chat.id, anchor: { message: message.id } }}><ContextMenu label={menuName} entries={entries}>
      <li
        id={`message-${message.id}`}
        data-message={message.id}
        aria-busy={pending || undefined}
        onClick={(event) => {
          if (selection.active && !(event.target as HTMLElement).closest('a,button')) selection.toggle(message.id, event.shiftKey);
        }}
        className={cx(
          'group relative flex gap-3 px-4 py-1 transition-colors hover:bg-bg-subtle focus-within:bg-bg-subtle',
          head && 'mt-(--message-gap) first:mt-0',
          lit === message.id && 'bg-accent-subtle hover:bg-accent-subtle',
          picked && 'bg-accent-subtle hover:bg-accent-subtle',
          pending && 'opacity-60',
          selection.active && 'cursor-pointer',
        )}
      >
        {selection.active && (
          <input
            type="checkbox"
            checked={picked}
            aria-label={t('chat.select.message', { name: person?.display_name ?? agent?.agent ?? '' })}
            onChange={() => undefined}
            onClick={(event) => {
              event.stopPropagation();
              selection.toggle(message.id, event.shiftKey);
            }}
            className="mt-1 size-4 shrink-0 accent-accent-solid"
          />
        )}
        <div className="flex w-(--avatar-size) shrink-0 justify-center pt-1">
          {head ? (
            agent !== null ? (
              <AgentAvatar />
            ) : null
          ) : (
            <span className="pt-1 text-xs whitespace-nowrap text-text-muted opacity-0 group-hover:opacity-100 group-focus-within:opacity-100" aria-hidden>
              {gutterTime(message.created_at, locale, timeZone)}
            </span>
          )}
        </div>
        <div className="min-w-0 flex-1">
          {head && (
            <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
              {agent !== null ? <AgentName author={agent} /> : person !== null && <UserChip user={person} size={28} className="-ml-[calc(var(--avatar-size)+var(--spacing-3))]" />}
              <RelativeTime at={message.created_at} className="shrink-0 text-xs text-text-muted" />
              {message.pinned !== null && (
                <span className="inline-flex items-center gap-1 text-xs text-text-muted">
                  <Pin size={16} aria-hidden />
                  <Trans i18nKey="chat.pinned.byChip" components={{ person: <UserChip user={people.get(message.pinned.by.id) ?? message.pinned.by} size={24} /> }} />
                </span>
              )}
            </div>
          )}
          {!head && message.pinned !== null && (
            <span className="inline-flex items-center gap-1 text-xs text-text-muted">
              <Pin size={16} aria-hidden />
              <Trans i18nKey="chat.pinned.byChip" components={{ person: <UserChip user={people.get(message.pinned.by.id) ?? message.pinned.by} size={24} /> }} />
            </span>
          )}
          {replied !== null && (
            <div className="mb-1 flex min-h-(--control-sm) max-w-full items-center gap-1 border-l-2 border-border-strong pl-2 text-sm text-text-muted">
              <CornerUpLeft size={16} aria-hidden className="shrink-0" />
              {replied.author !== null && 'id' in replied.author && <UserChip user={replied.author} size={24} />}
              <button type="button" onClick={() => acts.jump(replied.id)} aria-label={[t('chat.reply.open'), replied.author === null ? '' : 'id' in replied.author ? replied.author.display_name : replied.author.agent, replied.deleted ? t('chat.deleted') : mentionNames(replied.excerpt, people.values())].filter(Boolean).join(' · ')} className="focus-inset flex min-w-0 items-center gap-1 rounded-md text-left hover:text-text">
                {replied.author !== null && 'agent' in replied.author && <span className="truncate font-medium">{`${replied.author.agent}${replied.author.thread.redacted || replied.author.thread.name === null ? '' : ` · ${replied.author.thread.name}`}`}</span>}
                <span data-overflow-text={replied.deleted ? t('chat.deleted') : mentionNames(replied.excerpt, people.values())} className="truncate">{replied.deleted ? t('chat.deleted') : mentionNames(firstLine([{ type: 'text', text: replied.excerpt }], REPLY_EXCERPT_CHARS), people.values())}</span>
              </button>
            </div>
          )}
          {message.deleted ? (
            <p className="text-md text-text-muted italic">{t('chat.deleted')}</p>
          ) : isEditing ? (
            <MessageEditor message={message} />
          ) : (
            <div className="flex flex-col gap-2">
              <Parts message={message} />
              {(message.asked_agents.length > 0) && <AskedAgents asks={message.asked_agents} />}
              {message.edited_at !== null && <span className="text-xs text-text-muted">{t('chat.edited')}</span>}
            </div>
          )}
          <Reactions
            reactions={message.reactions}
            me={me}
            people={people}
            canReact={power.react}
            onToggle={(emoji) => acts.react(message, emoji)}
          />
        </div>
        {!selection.active && !isEditing && (
          <div
            className={cx(
              'absolute -top-3 right-4 flex items-center rounded-md border border-border bg-surface-raised p-1 shadow-sm',
              'opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100',
            )}
          >
            {power.react && <ReactButton onPick={(emoji) => acts.react(message, emoji)} />}
            {power.reply && (
              <IconButton size="sm" label={t('chat.actions.reply')} icon={<Reply aria-hidden />} onClick={() => acts.reply(message)} />
            )}
            <Menu
              label={menuName}
              align="end"
              entries={entries}
              trigger={<IconButton size="sm" action="actions" label={menuName} icon={<EllipsisVertical aria-hidden />} />}
            />
          </div>
        )}
      </li>
    </ContextMenu></PersonThingProvider>
  );
});

// ---------------------------------------------------------------- a message that has not reached the server

export function OutgoingMessage({ chatId, sent, author, onRestore }: { chatId: string; sent: Outgoing; author: UserRef | null; onRestore?: (() => void) | undefined }) {
  const { t } = useTranslation();
  const text = draftMentionText(sent.parts, sent.mentionNames ?? []);
  return (
    <li className="flex gap-3 px-4 py-1" data-outgoing={sent.clientId}>
      <div className="w-(--avatar-size) shrink-0" />
      <div className={cx('min-w-0 flex-1', sent.state === 'sending' && 'opacity-60')}>
        {author !== null && <UserChip user={author} size={28} className="-ml-[calc(var(--avatar-size)+var(--spacing-3))]" />}
        {text !== '' && <Body text={text} />}
        {sent.parts.some((part) => !isForwarded(part) && part.type === 'item_ref') && (
          <div className="mt-1 flex flex-col gap-1">
            {sent.parts.map((part, index) =>
              !isForwarded(part) && part.type === 'item_ref' ? <QuotedItem key={index} threadId={part.thread_id} itemId={part.item_id} /> : null,
            )}
          </div>
        )}
        {sent.state === 'sending' ? (
          <p className="text-xs text-text-muted">{t('chat.sending')}</p>
        ) : (
          <ErrorState compact error={sent.error} title={t('chat.notSent')} message={sent.error === null ? t('chat.notSent') : undefined} recovery={{ label: t('ui.error.review'), content: <div className="flex flex-wrap gap-2">
            {sent.agentReview !== undefined && onRestore !== undefined && <Button size="sm" onClick={onRestore}>{t('people.reviewAgain')}</Button>}
            <Button size="sm" onClick={() => resend(chatId, sent.clientId)}>
              {t('common.tryAgain')}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => discard(chatId, sent.clientId)}>
              {t('chat.discard')}
            </Button>
          </div> }} />
        )}
      </div>
    </li>
  );
}

