/** A message on its way to the agent, drawn at the end of the conversation as the person's message, dimmed until the agent takes it.
 *
 * It stands where the message will stand once the agent has taken it: the same gap before it as a row of the conversation has, and the
 * same block, so that the message does not move when its row replaces this one.
 */
import { useMutation } from '@tanstack/react-query';
import { Pencil, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { cancelInput, updateInput } from '@/api/threads';
import { editableWords, withEditedWords } from '@/features/composer/queue';
import { Part, PERSON_BLOCK } from '@/features/timeline/items/UserMessage';
import type { SentMessage } from '@/features/thread/sentMessages';
import type { ContentPart, ThreadInput } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { isComposing } from '@/lib/shortcuts';
import { threadReadOnly, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import type { ThreadReadRef } from '@/realtime/threadScope';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Textarea } from '@/ui/Textarea';
import { UserChip } from '@/ui/UserChip';

function PendingWords({ threadId, input }: { threadId: string; input: ThreadInput }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const me = useStore(client.session, (state) => state.userId);
  const may = useThreadPart(threadId, (view) => !threadReadOnly(view) && holds(view.thread?.access, 'thread.interact') && view.thread?.archived !== true);
  const [editing, setEditing] = useState(false);
  const [words, setWords] = useState('');
  const editor = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { if (editing) editor.current?.focus(); }, [editing]);
  const write = useMutation({
    mutationFn: async ({ target, id, content }: { target: ThreadReadRef; id: string; content: ContentPart[] | null }) => {
      if (content !== null) return updateInput(target, id, { content });
      return { input: await cancelInput(target, id), inputs: [] };
    },
    onSuccess: (reply, { target }) => { threadWriter.apply(target, { inputs: [...reply.inputs, reply.input] }); setEditing(false); },
  });
  const mine = may && input.author?.id === me && input.source === 'user';
  const pending = input.state === 'pending';
  const locked = !may || !pending || write.isPending;
  const content = withEditedWords(input.content, words);
  return <>
    <div className={cx(PERSON_BLOCK, !editing && 'opacity-60')}>
      <span className="sr-only">{t('thread.sent.waiting')}</span>
      {editing ? <form className="flex min-w-0 flex-col gap-2" onSubmit={(event) => {
        event.preventDefault();
        if (!locked && mine && content.length > 0) write.mutate({ target: read, id: input.id, content });
      }}>
        <Textarea ref={editor} label={t('composer.queue.editLabel')} hideLabel rows={2} value={words} disabled={locked || !mine} onChange={(event) => setWords(event.target.value)}
          onKeyDown={(event) => {
            if (isComposing(event.nativeEvent)) return;
            if (event.key === 'Escape' && !write.isPending) { event.preventDefault(); event.stopPropagation(); setEditing(false); }
            else if (event.key === 'Enter' && !event.shiftKey && !event.altKey) { event.preventDefault(); event.stopPropagation(); event.currentTarget.form?.requestSubmit(); }
          }} />
        <div className="flex items-center justify-end gap-1">
          <Button size="sm" type="submit" pending={write.isPending} disabled={locked || !mine || content.length === 0}>{t('composer.queue.save')}</Button>
          <Button size="sm" variant="ghost" disabled={write.isPending} onClick={() => setEditing(false)}>{t('common.cancel')}</Button>
        </div>
      </form> : input.content.map((part, index) => <Part key={index} part={part} thread={threadId} owner={input.author ?? undefined} updatedAt={input.created_at} />)}
    </div>
    {may && pending && !editing && <div className="flex items-center gap-1">
      {mine && <IconButton size="sm" label={t('composer.queue.editRow', { number: 1 })} icon={<Pencil aria-hidden />} disabled={locked} onClick={() => { setWords(editableWords(input.content)); setEditing(true); }} />}
      <IconButton size="sm" label={t('composer.queue.removeRow', { number: 1 })} icon={<X aria-hidden />} disabled={locked} onClick={() => { if (!locked) write.mutate({ target: read, id: input.id, content: null }); }} />
    </div>}
    {write.error !== null && <ErrorAlert error={write.error} />}
  </>;
}

export function SentMessageBlock({ threadId, message }: { threadId: string; message: SentMessage }) {
  const { t } = useTranslation();
  return (
    <div className="mx-auto w-full max-w-(--reading-width) px-4" style={{ paddingTop: 'calc(var(--message-gap) / 2)', paddingBottom: 'calc(var(--message-gap) / 2)' }} data-sent-message={message.key}>
      <div className="mt-(--message-gap) flex min-w-0 flex-col items-end gap-1">
        {message.input?.author !== null && message.input?.author !== undefined && <UserChip user={message.input.author} size={28} />}
        {message.input === null ? <div className={cx(PERSON_BLOCK, 'opacity-60')}>
          <span className="sr-only">{t('thread.sent.waiting')}</span>
          {message.content.map((part, index) => <Part key={index} part={part} thread={threadId} updatedAt={message.at} />)}
        </div> : <PendingWords threadId={threadId} input={message.input} />}
      </div>
    </div>
  );
}
