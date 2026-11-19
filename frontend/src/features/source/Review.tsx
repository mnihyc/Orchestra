/** Comments on lines of a difference: written privately into the thread's draft with the side, the lines and the hunk as they were read, gathered in a tray above the box, and sent once as one message to the agent.
 *
 * A comment keeps the hunk it was written on, so that a difference that changed since (the agent wrote
 * again, or the file was restored) does not move it: the tray says so instead. Comments live in the
 * person's draft on the server, so another device shows the same; a save that fails keeps the words in
 * the form. The message they make is shown before it is sent, and goes through the ordinary input.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { MessageSquareCode, Pencil, X } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { saveReviewComments } from '@/api/source';
import { draftQuery, sendInput, threadKeys } from '@/api/threads';
import type { Draft, ReviewComment, ReviewSide } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { graphemeCount } from '@/lib/grapheme';
import { newActKey } from '@/lib/requests';
import { threadWriter, useThreadRow } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';

import { hunkRange, hunksOf, reviewedHash, reviewMessage } from './words';

/** The longest comment, in characters: a review note, not a document. */
const COMMENT_MOST = 4_000;

/** Writes the draft's comments anew from what the server holds now, so that another device's comments are kept. */
async function withComments(threadId: string, change: (comments: ReviewComment[]) => ReviewComment[], current: Draft | null): Promise<Draft> {
  const comments = change(current?.review_comments ?? []);
  return saveReviewComments(threadId, current?.content ?? [], current?.delivery ?? 'queue', comments);
}

/** "Comment on lines…" beside a difference, for whoever may talk to the thread's agent. */
export function CommentOnLines({ threadId, path, diff, itemId }: { threadId: string; path: string; diff: string; itemId: string | null }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const thread = useThreadRow(threadId);
  const hunks = hunksOf(diff);
  if (thread === null || !holds(thread.access, 'thread.interact')) return null;
  return (
    <>
      <IconButton size="sm" label={t('source.review.comment')} icon={<MessageSquareCode aria-hidden />} disabled={hunks.length === 0} onClick={() => setOpen(true)} />
      {open && <CommentDialog threadId={threadId} path={path} diff={diff} itemId={itemId} onClose={() => setOpen(false)} />}
    </>
  );
}

function CommentDialog({ threadId, path, diff, itemId, onClose }: { threadId: string; path: string; diff: string; itemId: string | null; onClose: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const hunks = hunksOf(diff);
  const [at, setAt] = useState('0');
  const [side, setSide] = useState<ReviewSide>('new');
  const hunk = hunks[Number(at)] ?? hunks[0];
  const range = hunk === undefined ? { from: 1, to: 1 } : hunkRange(hunk, side);
  const [from, setFrom] = useState(range.from);
  const [to, setTo] = useState(range.to);
  const [text, setText] = useState('');
  const save = useMutation({
    mutationFn: async () => {
      if (hunk === undefined) return null;
      const current = await client.query({ ...draftQuery(threadId), staleTime: 0 });
      const comment: ReviewComment = { id: crypto.randomUUID(), path, side, from_line: from, to_line: to, diff_hash: await reviewedHash(diff), item_id: itemId, hunk: hunk.text, text: text.trim() };
      return withComments(threadId, (comments) => [...comments, comment], current);
    },
    onSuccess: (draft) => {
      if (draft !== null) client.setQueryData(threadKeys.draft(threadId), draft);
      onClose();
    },
  });
  const choose = (next: { at?: string; side?: ReviewSide }) => {
    const nextHunk = hunks[Number(next.at ?? at)];
    const nextSide = next.side ?? side;
    if (next.at !== undefined) setAt(next.at);
    if (next.side !== undefined) setSide(next.side);
    if (nextHunk !== undefined) {
      const bounds = hunkRange(nextHunk, nextSide);
      setFrom(bounds.from);
      setTo(bounds.to);
    }
  };
  const bounds = hunk === undefined ? null : hunkRange(hunk, side);
  const inside = bounds !== null && from >= bounds.from && to <= bounds.to && from <= to;
  const ready = inside && text.trim() !== '' && graphemeCount(text) <= COMMENT_MOST;
  return (
    <Dialog
      open
      size="md"
      title={t('source.review.title', { path })}
      description={t('source.review.lead')}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button action="save" variant="primary" pending={save.isPending} disabled={!ready} onClick={() => save.mutate()}>
            {t('source.review.save')}
          </Button>
        </>
      }
    >
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (ready) save.mutate(); }}>
        <Select<string> label={t('source.review.hunk')} value={at} onValueChange={(value) => choose({ at: value })} options={hunks.map((one, index) => ({ value: String(index), label: one.text.split('\n')[0] ?? '' }))} />
        <RadioGroup<ReviewSide>
          variant="list"
          label={t('source.review.sideLabel')}
          value={side}
          onValueChange={(value) => choose({ side: value })}
          options={[
            { value: 'new', ...choiceOption(t, 'reviewSide.new') },
            { value: 'old', ...choiceOption(t, 'reviewSide.old') },
          ]}
        />
        <div className="flex gap-2">
          <Input label={t('source.review.from')} type="number" inputMode="numeric" min={bounds?.from} max={bounds?.to} value={String(from)} onChange={(event) => setFrom(Number(event.target.value))} error={inside ? undefined : t('source.review.outside', { from: bounds?.from ?? 1, to: bounds?.to ?? 1 })} />
          <Input label={t('source.review.to')} type="number" inputMode="numeric" min={bounds?.from} max={bounds?.to} value={String(to)} onChange={(event) => setTo(Number(event.target.value))} />
        </div>
        {hunk !== undefined && <pre className="max-h-40 overflow-auto rounded-lg border border-border bg-bg-subtle p-2 font-mono text-xs">{hunk.text}</pre>}
        <Textarea label={t('source.review.text')} value={text} rows={4} onChange={(event) => setText(event.target.value)} hint={t('source.review.private')} />
        {save.error !== null && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}

/** The comments waiting in the draft, above the box: each editable and removable, and Review message… to send them as one. */
export function ReviewTray({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const draft = useQuery({ ...draftQuery(threadId), retry: false });
  const [editing, setEditing] = useState<ReviewComment | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const remove = useMutation({
    mutationFn: async (id: string) => withComments(threadId, (comments) => comments.filter((comment) => comment.id !== id), await client.query({ ...draftQuery(threadId), staleTime: 0 })),
    onSuccess: (saved) => client.setQueryData(threadKeys.draft(threadId), saved),
  });
  const comments = draft.data?.review_comments ?? [];
  if (comments.length === 0) return null;
  return (
    <section aria-label={t('source.review.tray')} className="flex flex-col gap-1 rounded-lg border border-border bg-bg-subtle px-3 py-2 text-sm" data-review-tray>
      <div className="flex flex-wrap items-center gap-2">
        <MessageSquareCode aria-hidden className="size-4 text-text-muted" />
        <span className="flex-1 font-medium">{t('source.review.waiting', { count: comments.length })}</span>
        <Button size="sm" variant="primary" icon={<MessageSquareCode aria-hidden />} onClick={() => setPreviewing(true)}>
          {t('source.review.preview')}
        </Button>
      </div>
      <ul className="flex flex-col gap-1">
        {comments.map((comment) => (
          <li key={comment.id} className="flex items-start gap-2" data-review-comment={comment.id}>
            <span className="min-w-0 flex-1">
              <code className="wrap-anywhere">{comment.from_line === comment.to_line ? `${comment.path}:${comment.from_line}` : `${comment.path}:${comment.from_line}-${comment.to_line}`}</code>
              <span className="ml-2 text-text-muted">{t(`source.review.side.${comment.side}`)}</span>
              <span className="block truncate text-text-muted">{comment.text}</span>
            </span>
            <IconButton size="sm" label={t('source.review.edit')} icon={<Pencil aria-hidden />} onClick={() => setEditing(comment)} />
            <IconButton size="sm" label={t('source.review.remove')} icon={<X aria-hidden />} pending={remove.isPending && remove.variables === comment.id} onClick={() => remove.mutate(comment.id)} />
          </li>
        ))}
      </ul>
      {remove.error !== null && <ErrorAlert error={remove.error} />}
      {editing !== null && <EditComment threadId={threadId} comment={editing} onClose={() => setEditing(null)} />}
      {previewing && <SendReview threadId={threadId} comments={comments} onClose={() => setPreviewing(false)} />}
    </section>
  );
}

function EditComment({ threadId, comment, onClose }: { threadId: string; comment: ReviewComment; onClose: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [text, setText] = useState(comment.text);
  const save = useMutation({
    mutationFn: async () => withComments(threadId, (comments) => comments.map((one) => (one.id === comment.id ? { ...one, text: text.trim() } : one)), await client.query({ ...draftQuery(threadId), staleTime: 0 })),
    onSuccess: (saved) => {
      client.setQueryData(threadKeys.draft(threadId), saved);
      onClose();
    },
  });
  const ready = text.trim() !== '' && graphemeCount(text) <= COMMENT_MOST;
  return (
    <Dialog open size="md" title={t('source.review.editTitle', { path: comment.path })} onOpenChange={(next) => { if (!next) onClose(); }} footer={<><Button onClick={onClose}>{t('common.cancel')}</Button><Button action="save" variant="primary" pending={save.isPending} disabled={!ready} onClick={() => save.mutate()}>{t('source.review.save')}</Button></>}>
      <div className="flex flex-col gap-3">
        <pre className="max-h-40 overflow-auto rounded-lg border border-border bg-bg-subtle p-2 font-mono text-xs">{comment.hunk}</pre>
        <Textarea label={t('source.review.text')} value={text} rows={4} onChange={(event) => setText(event.target.value)} />
        {save.error !== null && <ErrorAlert error={save.error} />}
      </div>
    </Dialog>
  );
}

function SendReview({ threadId, comments, onClose }: { threadId: string; comments: ReviewComment[]; onClose: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const key = useRef(newActKey());
  const text = reviewMessage(t, comments);
  const send = useMutation({
    mutationFn: async () => {
      const reply = await sendInput(threadId, { content: [{ type: 'text', text }], delivery: 'queue', clientId: key.current });
      threadWriter.apply(threadId, { inputs: [reply.input] });
      // What was sent leaves the draft; comments written meanwhile on another device stay.
      const sent = new Set(comments.map((comment) => comment.id));
      const saved = await withComments(threadId, (all) => all.filter((comment) => !sent.has(comment.id)), await client.query({ ...draftQuery(threadId), staleTime: 0 }));
      client.setQueryData(threadKeys.draft(threadId), saved);
    },
    onSuccess: onClose,
  });
  return (
    <Dialog
      open
      size="lg"
      title={t('source.review.sendTitle')}
      description={t('source.review.sendLead', { count: comments.length })}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button action="send" variant="primary" pending={send.isPending} onClick={() => send.mutate()}>
            {t('source.review.send')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2">
        <pre className="max-h-96 overflow-auto rounded-lg border border-border bg-bg-subtle p-3 font-mono text-xs whitespace-pre-wrap wrap-anywhere" data-review-message>
          {text}
        </pre>
        {send.error !== null && <ErrorAlert error={send.error} />}
      </div>
    </Dialog>
  );
}
