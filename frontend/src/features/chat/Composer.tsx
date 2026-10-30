/** IME-aware chat input with mentions, emoji, replies, quotes and a draft; attaching waits for the server.
 *
 * The box under a chat's messages. It grows from one line to eight; Enter sends and Shift+Enter is a new
 * line; Up in an empty box edits the reader's last message; an `@` opens the people who can read the
 * chat with "@all" first; an emoji can be put in; a reply and the items quoted from a thread stand
 * above it as bars that can be taken away. While an input method is composing, Enter never sends. What
 * is typed is local first and kept as a draft. Files use resumable uploads and references retain
 * their origin. Where the reader may not write, a sentence stands in its place.
 */
import { useQuery } from '@tanstack/react-query';
import { CornerUpLeft, Paperclip, SendHorizontal, Smile, X } from 'lucide-react';
import { type KeyboardEvent, type RefObject, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';

import { mentionAudienceQuery, mentionChoicesQuery } from '@/api/people';
import { AgentAsks, AgentAskError } from '@/features/people/AgentAsks';
import { labelPlaceMention } from '@/features/people/mentionParts';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { BROAD_AUDIENCE, candidates, MentionPicker, type Candidate } from '@/features/people/MentionPicker';
import { mentionNames } from '@/features/people/wording';
import { send } from '@/features/chat/acts';
import { useChatContext } from '@/features/chat/context';
import { type Draft, partsOfDraft } from '@/features/chat/draft';
import { AttachmentChips, UploadChips } from '@/features/composer/Attachments';
import { UploadHelp } from '@/features/composer/UploadHelp';
import { DropTarget } from '@/features/composer/DropTarget';
import { carriesFiles, droppedReference, insertAtSelection } from '@/lib/drop';
import { useFileDrops } from '@/features/composer/FileDrops';
import { useViewShown } from '@/features/workbench/viewOpening';
import { QuotedItem } from '@/features/chat/QuotedItem';
import { firstLine, insertMention, isBlank, type MentionQuery, mentionAt, textOfParts } from '@/features/chat/text';
import type { Chat, ChatMessage, NodeRef } from '@/generated/api-types';
import { isComposing } from '@/lib/shortcuts';
import { attachToChat, releaseUpload } from '@/lib/uploads';
import { usePastedText } from '@/features/composer/usePastedText';
import { useUploads } from '@/lib/useUploads';
import { tellTyping, useChatEntry } from '@/realtime/stores/chat';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { EmojiPicker } from '@/ui/EmojiPicker';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';
import { UserChip } from '@/ui/UserChip';

/** The most lines the box grows to before it scrolls. */
const MAX_LINES = 8;
/** How many characters of the message it answers the reply bar shows. */
const REPLY_BAR_CHARS = 100;

export interface ComposerProps {
  chat: Chat;
  draft: Draft;
  /** The message the draft answers, for the bar above the box. */
  replyingTo: ChatMessage | null;
  /** The box, so that something outside can put the focus in it. */
  inputRef: RefObject<HTMLTextAreaElement | null>;
  /** Up in an empty box: edit the last message the reader wrote. */
  onEditLast: () => void;
  /** What stands above the box in a chat of a thread: the number of items that can be quoted is shown by the caller. */
  placeholder: string;
  onAsk?: (thread: NodeRef, agent: string, message: ChatMessage) => void;
}

export function Composer({ chat, draft, replyingTo, inputRef, onEditLast, placeholder, onAsk }: ComposerProps) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const { chat: current } = useChatContext();
  const knownPlaces = useKnownPlaces();
  const entry = useChatEntry(chat.id);
  const listId = useId();
  const [caret, setCaret] = useState(0);
  const [active, setActive] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [emoji, setEmoji] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const uploads = useUploads(chat.id, (blob, signal) => attachToChat(chat.id, blob, signal), draft.addAttachment);
  const fileDrops = useFileDrops(`chat:${chat.id}`);
  const removePart = (index: number) => {
    const part = draft.attachments[index];
    if (part !== undefined && 'blob' in part) void releaseUpload(part.blob).catch(() => undefined);
    if (part?.type === 'pasted_text') pastes.cancel(part);
    draft.removeAttachment(index);
  };
  const pastes = usePastedText({ kind: 'chat', id: chat.id, box: inputRef, maxLines: MAX_LINES, text: draft.text, setText: draft.setText,
    add: draft.addAttachment, replace: (original, part) => draft.replaceAttachment?.(original, part), remove: removePart });

  const mention: MentionQuery | null = mentionAt(draft.text, caret);
  const open = mention !== null && dismissed !== `${mention.start}:${mention.query}`;
  const found = useQuery({ ...mentionChoicesQuery(chat.id, mention?.query ?? ''), enabled: shown && open });
  const options = candidates(found.data?.items ?? [], found.data?.followers ?? 0, mention?.query ?? '',(choice) => labelPlaceMention(choice.target,choice.name,knownPlaces,(role,place) => role === 'viewer' ? t('access.holder.everyoneIn',{place}) : t('access.holder.roleOf',{roles:t(`access.holder.roles.${role}`),place})));
  const parts = partsOfDraft(draft);
  const hasMention = draft.text.includes('@') || parts.some((part) => part.type === 'mention');
  const agentMention = parts.some((part) => part.type === 'mention' && part.target.kind === 'agent');
  const audience = useQuery({ ...mentionAudienceQuery(chat.id, parts, draft.replyTo), enabled: shown && hasMention });
  const confirming = hasMention && (!audience.isSuccess || audience.isFetching);
  const sendLabel = hasMention && (audience.data?.count ?? 0) > BROAD_AUDIENCE ? t('people.sendTell', { count: audience.data?.count }) : t('chat.send');
  const replyAuthor = replyingTo?.author;
  const agentReply = replyAuthor !== null && replyAuthor !== undefined && 'agent' in replyAuthor ? replyAuthor : null;
  const namedAgent = current.agents?.find((agent) => draft.text.toLowerCase().includes(`@${agent.agent.toLowerCase()}`));
  const asked = agentReply ?? namedAgent ?? null;
  const related = replyingTo ?? (asked === null ? null : [...entry.messages.values()].reverse().find((message) => message.author !== null && 'agent' in message.author && message.author.thread.id === asked.thread.id && !message.deleted) ?? null);
  const chosen = Math.min(active, Math.max(0, options.length - 1));

  // The box is as tall as its text, up to eight lines.
  useLayoutEffect(() => {
    const element = inputRef.current;
    if (element === null) return;
    element.style.height = 'auto';
    const style = getComputedStyle(element);
    const line = Number.parseFloat(style.lineHeight) || 24;
    const padding = (Number.parseFloat(style.paddingTop) || 0) + (Number.parseFloat(style.paddingBottom) || 0);
    const border = element.offsetHeight - element.clientHeight;
    // scrollHeight already includes padding; the eight-line cap needs that padding only once too.
    element.style.height = `${Math.min(element.scrollHeight + border, line * MAX_LINES + padding + border)}px`;
  }, [draft.text, inputRef]);

  // What is typed is told to the others at most as often as the protocol lets a frame through.
  useEffect(() => {
    if (!isBlank(draft.text)) tellTyping(chat.id);
  }, [draft.text, chat.id]);

  const submit = () => {
    const parts = partsOfDraft(draft);
    if (parts.length === 0 || uploads.blocked || pastes.blocked || fileDrops.blocked || confirming || (agentMention && typeof audience.data?.agent_review !== 'string')) return;
    if (agentMention && typeof audience.data?.agent_review === 'string') send(chat.id, parts, draft.replyTo, draft.mentions, audience.data.agent_review);
    else if ((draft.mentions?.length ?? 0) > 0) send(chat.id, parts, draft.replyTo, draft.mentions);
    else send(chat.id, parts, draft.replyTo);
    draft.clear();
    setCaret(0);
  };

  const choose = (option: Candidate) => {
    if (mention === null || !option.available) return;
    const done = insertMention(draft.text, mention, option.name);
    draft.setText(done.text);
    if (option.choice !== null) draft.addMention?.({ name: option.name, target: option.choice.target }, mention.start);
    setCaret(done.caret);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.setSelectionRange(done.caret, done.caret);
    });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (isComposing(event.nativeEvent)) return;
    if (open && options.length > 0) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        setActive((chosen + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length);
        return;
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault();
        const option = options[chosen];
        if (option !== undefined) choose(option);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        setDismissed(`${mention.start}:${mention.query}`);
        return;
      }
    }
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      submit();
      return;
    }
    if (event.key === 'ArrowUp' && draft.text === '' && event.currentTarget.selectionStart === 0) {
      event.preventDefault();
      onEditLast();
      return;
    }
    if (event.key === 'Escape' && draft.replyTo !== null) {
      event.preventDefault();
      draft.setReplyTo(null);
    }
  };

  const empty = partsOfDraft(draft).length === 0;
  const uploadStatus = uploads.blocked || pastes.blocked ? t('uploads.waiting') : '';
  const draftStatus = draft.saving === 'saving' ? t('chat.draft.saving') : draft.saving === 'saved' ? t('chat.draft.saved') : draft.saving === 'failed' ? t('chat.draft.failed') : '';
  const hasStatus = uploadStatus !== '' || draftStatus !== '';
  return (
    <div data-chat-composer className="flex min-w-0 shrink-0 flex-col gap-1 border-t border-border bg-bg px-3 py-2">
      <input ref={fileInput} type="file" multiple hidden aria-label={t('uploads.choose')} onChange={(event) => { if (event.target.files !== null) uploads.add(event.target.files); event.target.value = ''; }} />
      <UploadChips queue={uploads} />
      {fileDrops.progress}
      <UploadChips queue={pastes.queue} />
      <AttachmentChips parts={draft.attachments} onRemove={removePart} destination={() => t('uploads.chatFiles')}
        readPaste={pastes.read} pastedIdentity={pastes.identity} onPasteAsText={pastes.asText} onPasteDirty={pastes.markDirty} editText={pastes.editText} editing={pastes.editing} pasteQueue={pastes.queue} onPasteDelivery={pastes.delivery}
        onEditPaste={draft.replaceAttachment === undefined ? undefined : (index, words) => { const part = draft.attachments[index]; if (part?.type === 'pasted_text') pastes.edit(part, words); }} />
      {replyingTo !== null && (
        <div className="flex items-center gap-2 rounded-lg border-l-2 border-accent bg-bg-subtle py-1 pr-1 pl-3 text-sm">
          <CornerUpLeft size={16} aria-hidden className="shrink-0 text-text-muted" />
          <span data-overflow-text={replyingTo.deleted ? t('chat.deleted') : mentionNames(textOfParts(replyingTo.parts), replyingTo.mentions)} className="min-w-0 flex-1 truncate">
            <span className="font-medium">{replyingTo.author !== null && 'id' in replyingTo.author
              ? <Trans i18nKey="chat.reply.toChip" components={{ person: <UserChip user={replyingTo.author} size={24} thing={{ kind: 'chat', id: chat.id, anchor: { message: replyingTo.id } }} /> }} />
              : t('chat.reply.to', { name: replyingTo.author?.agent ?? '' })}</span>
            <span className="text-text-muted"> · {replyingTo.deleted ? t('chat.deleted') : mentionNames(firstLine(replyingTo.parts, REPLY_BAR_CHARS), replyingTo.mentions)}</span>
          </span>
          <IconButton size="sm" label={t('chat.reply.cancel')} icon={<X aria-hidden />} onClick={() => draft.setReplyTo(null)} />
        </div>
      )}
      {draft.quotes.length > 0 && (
        <div className="flex flex-col gap-1">
          {draft.quotes.map((quote, index) => (
            <QuotedItem key={quote.itemId} threadId={quote.threadId} itemId={quote.itemId} onRemove={() => draft.removeQuote(index)} />
          ))}
        </div>
      )}
      {asked !== null && <p className="flex flex-wrap items-center gap-2 text-xs text-text-muted"><span>{t('people.askHint', { agent: asked.agent })}</span>{onAsk !== undefined && related !== null && !asked.thread.redacted && <Button size="sm" onClick={() => onAsk(asked.thread, asked.agent, related)}>{t('people.askIt')}</Button>}</p>}
      {hasMention && audience.isError && (agentMention ? <AgentAskError error={audience.error} /> : <ErrorAlert error={audience.error} />)}
      {agentMention && audience.isSuccess && !audience.isFetching && <AgentAsks chatId={chat.id} asks={audience.data.agent_asks ?? []} />}
      <div className="relative">
        <span id={`${listId}-hint`} className="sr-only">
          {t('chat.composer.hint')}
        </span>
        {open && <MentionPicker id={listId} options={options} active={chosen} onChoose={choose} pending={found.isPending} error={found.error} />}
        <DropTarget label={t('uploads.target.chat')} countLabel={(values) => t('uploads.target.chatCount', values)} onDrop={(data) => {
          const reference = droppedReference(data);
          if (reference !== null) {
            const next = insertAtSelection(draft.text, reference, inputRef.current);
            draft.setText(next.text); setCaret(next.caret);
            requestAnimationFrame(() => { inputRef.current?.focus(); inputRef.current?.setSelectionRange(next.caret, next.caret); });
          } else if (carriesFiles(data)) fileDrops.read(data, t('uploads.target.chat'), (files) => uploads.add(files));
        }}>
        <div role="group" data-control-boundary aria-label={t('chat.composer.controls')} className="flex min-w-0 items-end gap-1 rounded-lg border border-control-border bg-surface p-1 focus-within:border-focus">
          <textarea
            ref={inputRef}
            rows={1}
            value={draft.text}
            aria-label={placeholder}
            placeholder={t('chat.composer.placeholder')}
            aria-describedby={`${listId}-hint`}
            aria-autocomplete="list"
            aria-controls={open && options.length > 0 ? listId : undefined}
            aria-activedescendant={open && options.length > 0 ? `${listId}-${chosen}` : undefined}
            onChange={(event) => {
              draft.setText(event.target.value);
              setCaret(event.target.selectionStart);
              setActive(0);
              setDismissed(null);
            }}
            onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
            onKeyDown={onKeyDown}
            onPaste={(event) => {
              if (event.clipboardData.files.length > 0) {
                event.preventDefault();
                fileDrops.read(event.clipboardData, t('uploads.target.chat'), (files) => uploads.add(files));
              } else if (pastes.paste(event.clipboardData.getData('text/plain'))) event.preventDefault();
            }}
            className="min-h-8 min-w-0 flex-1 resize-none bg-transparent px-2 py-1 text-md text-text outline-none placeholder:text-text-subtle"
          />
          <IconButton size="sm" label={t('chat.attach.label')} icon={<Paperclip aria-hidden />} onClick={() => fileInput.current?.click()} />
          <UploadHelp topic="message-files" />
          <Popover
            label={t('chat.emoji.label')}
            scroll="children"
            open={emoji}
            onOpenChange={setEmoji}
            side="top"
            align="end"
            trigger={<IconButton size="sm" label={t('chat.emoji.label')} icon={<Smile aria-hidden />} />}
          >
            <EmojiPicker
              onPick={(picked) => {
                const element = inputRef.current;
                const at = element?.selectionStart ?? draft.text.length;
                const end = element?.selectionEnd ?? at;
                draft.setText(`${draft.text.slice(0, at)}${picked}${draft.text.slice(end)}`);
                setEmoji(false);
                requestAnimationFrame(() => {
                  element?.focus();
                  element?.setSelectionRange(at + picked.length, at + picked.length);
                });
              }}
            />
          </Popover>
          <IconButton size="sm" variant="secondary" label={sendLabel} title={sendLabel} icon={<SendHorizontal aria-hidden />} disabled={empty || uploads.blocked || pastes.blocked || fileDrops.blocked || confirming || (agentMention && typeof audience.data?.agent_review !== 'string')} onClick={submit} />
        </div>
        </DropTarget>
      </div>
      <div role="status" className={cx('text-xs text-text-subtle', hasStatus ? 'flex flex-wrap items-center justify-between gap-x-2' : 'sr-only')}>
        {uploadStatus !== '' && <span>{uploadStatus}</span>}
        {draftStatus !== '' && <span className="inline-flex items-center gap-2">{draftStatus}{draft.saving === 'failed' && draft.retry !== undefined && <Button size="sm" variant="ghost" onClick={draft.retry}>{t('common.tryAgain')}</Button>}</span>}
      </div>
    </div>
  );
}
