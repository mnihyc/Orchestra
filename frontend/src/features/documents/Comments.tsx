/** Discussions beside the text: a card at the height of its block with the words it is about, answers, reactions and Resolve; the list of them all; and what becomes of one whose text changed or went.
 *
 * Talk about a place in a document stands beside that place. Selecting some words and choosing Comment,
 * or Comment in a block's menu, opens a discussion; people answer in it, react to a comment and resolve
 * it. The panel lists them all: open, resolved or both, in the order of the page or by the latest answer;
 * choosing one scrolls to its place. A discussion whose words were rewritten stays on its block, and one
 * whose block is gone is listed under "On text that was removed". Comments are beside the document and no
 * part of it: they are in no version.
 */
import { Check, MessageSquare, MessageSquarePlus, Pencil, RotateCcw, SendHorizontal, Trash2 } from 'lucide-react';
import { createContext, memo, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useStore } from 'zustand';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { commentPageQuery, discussionPageQuery } from '@/api/commentPages';
import { documentNoticeJobKey } from '@/api/documentNoticeJobs';
import { DocumentNoticeJobs } from '@/features/people/DocumentNoticeJobs';
import { Trans, useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { ApiError, client } from '@/api/client';
import {
  answerDiscussion,
  deleteComment,
  editComment,
  reactToComment,
  resolveDiscussion,
  startDiscussion,
  staleEditedComment,
  type Document,
  type DocumentComment,
  type DocumentDiscussion,
} from '@/api/documents';
import { buildTree, preOrder, plainText } from '@/features/documents/blocks';
import { highlightBlock, scrollToDocumentBlock, useDocumentMarksScope, useDocumentMarksStore } from '@/features/documents/marks';
import { beginCommentSend, commentComposeSource, rememberCommentComposeSource, commentDraftLossTargets, commentIntent, commentReader, commentSlotKey, currentCommentReader, discardComment,
  finishCommentSend, getCommentDrafts, markCommentConflict, reviewCommentBase, saveCommentDrafts, selectCommentText, setCommentText, useCommentDraftGuard, useCommentDrafts, useCommentDraftLossTargets,
  type CommentDrafts, type CommentMention, type CommentRun, type CommentTarget } from '@/features/documents/commentDrafts';
import { useViewInstance } from '@/features/workbench/instance';
import { findTab, mapGroups } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { sources } from '@/features/documents/BlockMenu';
import { CommentField } from '@/features/people/CommentField';
import { useListChoice } from '@/lib/listState';
import { PersonThingProvider } from '@/features/people/personContext';
import type { Inline } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { NetworkError } from '@/lib/requests';
import { documentStateNow, documentWriter, useDocumentState, useDocumentStore } from '@/realtime/stores/document';
import { UserChip } from '@/ui/UserChip';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ReactButton } from '@/ui/EmojiPicker';
import { ErrorAlert } from '@/ui/ErrorState';
import { Dialog } from '@/ui/Dialog';
import { InlineAlert } from '@/ui/InlineAlert';
import { IconButton } from '@/ui/IconButton';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';

type Show = 'open' | 'resolved' | 'all';
type Order = 'page' | 'latest';

/** How many discussions beside the document nobody has resolved. */
export function useCommentCount(docId: string): number {
  const state = useDocumentState(docId);
  const loaded = [...state.discussions.values()].filter((one) => one.resolved === null).length;
  return state.discussions.size < 50 ? loaded : (state.document?.open_discussions ?? loaded);
}

/** Tells the page where the words of each open discussion are, so that they are marked in the text. */
export function useCommentAnchors(docId: string): void {
  const marks = useDocumentMarksStore();
  const state = useDocumentState(docId);
  useEffect(() => {
    const anchors = [...state.discussions.values()]
      .filter((one) => one.resolved === null)
      .map((one) => ({ discussion: one.id, block: one.block_id, quote: one.anchored ? one.quote : '', open: true }));
    marks.setState({ anchors });
    return () => marks.setState({ anchors: [] });
  }, [state.discussions, marks]);
}

// ---------------------------------------------------------------- text

/** The words of a comment, with their marks and links. */
export function CommentText({ content }: { content: readonly Inline[] }) {
  return (
    <p className="text-sm break-words whitespace-pre-wrap text-text">
      {content.map((run, index) => {
        // Inline content is given whole and never reordered.
        if (run.type === 'text') {
          const link = typeof run.href === 'string' && run.href !== '';
          const inner = <span className={cx(run.marks?.includes('bold') && 'font-semibold', run.marks?.includes('italic') && 'italic', run.marks?.includes('code') && 'rounded-sm bg-bg-muted px-1 font-mono text-xs')}>{run.text}</span>;
          return link ? (
            <a key={index} href={run.href ?? undefined} rel="noreferrer noopener" target="_blank" className="text-accent-text underline">
              {inner}
            </a>
          ) : (
            <span key={index}>{inner}</span>
          );
        }
        if (run.type === 'mention') return run.user === null || run.user === undefined ? <span key={index}>@</span> : <UserChip key={index} user={run.user} mention size={24} className="rounded-md bg-accent-subtle px-1 align-baseline" />;
        if (run.type === 'break') return <br key={index} />;
        return <span key={index}>{plainText([run])}</span>;
      })}
    </p>
  );
}

/** What was typed as the content of a comment: lines, and the people named with an `@`. */
export function inlineOf(text: string, readers: readonly CommentMention[] = sources.people): CommentRun[] {
  const out: CommentRun[] = [];
  text.split('\n').forEach((line, row) => {
    if (row > 0) out.push({ type: 'break' });
    let from = 0;
    for (const at of line.matchAll(/(^|\s)@([a-z0-9_-]{1,32})(?![a-z0-9_-])/gi)) {
      const person = readers.find((one) => one.username.toLowerCase() === at[2]?.toLowerCase());
      if (person === undefined) continue;
      const start = at.index + (at[1]?.length ?? 0);
      if (start > from) out.push({ type: 'text', text: line.slice(from, start) });
      out.push({ type: 'mention', user_id: person.id });
      from = at.index + at[0].length;
    }
    if (from < line.length) out.push({ type: 'text', text: line.slice(from) });
  });
  return out;
}

// ---------------------------------------------------------------- the composer

/** A late local-write failure in a previously followed document must retain a visible recovery entrance. */
export function CommentRecoveryNotice({ displayedDocument }: { displayedDocument: string | undefined }) {
  const { t } = useTranslation();
  const instance = useViewInstance();
  const reader = commentReader();
  const targets = useCommentDraftLossTargets(instance?.id);
  const documents = useDocumentStore((state) => state.byId);
  const other = targets.filter((id) => id !== displayedDocument);
  if (other.length === 0 || instance === null) return null;
  return <div className="shrink-0 p-3"><ErrorAlert error={null} message={t('documents.comments.localSaveFailed')}
    action={<div className="flex flex-wrap gap-2">{other.map((documentId) => {
      const state = documents.get(documentId);
      const title = state?.ended === null && state.error === null ? state.document?.title : undefined;
      return <Button key={documentId} size="sm" variant="ghost" onClick={() => {
        if (!currentCommentReader(reader) || !commentDraftLossTargets(instance.id).includes(documentId)) return;
        const current = useWorkbench.getState();
        if (findTab(current.layout, instance.id)?.tab.view !== 'comments') return;
        current.change((layout) => mapGroups(layout, (group) => ({ ...group, tabs: group.tabs.map((tab) => tab.id === instance.id
          ? { ...tab, of: undefined, scope: { mode: 'fixed' as const, target: { kind: 'document' as const, id: documentId } } } : tab) })));
      }}>{t('documents.comments.recoverOther', { title: title ?? t('documents.comments.recoverDocument') })}</Button>;
    })}</div>} /></div>;
}

const DraftContext = createContext<CommentDrafts | null>(null);
function useRecovery(): CommentDrafts {
  const controller = useContext(DraftContext);
  if (controller === null) throw new Error('document comment recovery scope missing');
  return controller;
}
function mentionedReaders(text: string, held: readonly CommentMention[], found: readonly CommentMention[]): CommentMention[] {
  const used = found.filter((user) => text.toLowerCase().includes(`@${user.username.toLowerCase()}`));
  // Keep previously used bindings, as the original composer did, even while
  // text is temporarily edited away and back before retrying a lost answer.
  return [...new Map([...held, ...used].map((user) => [user.id, { id: user.id, username: user.username }])).values()];
}

function Composer<Result>({ docId, target, blockId, writable, sendable = writable, placeholder, label, onSend, accepts, onAccepted, onCancel, initial = '', autofocus = false, editVersion }:
  { docId: string; target: CommentTarget; blockId?: string | null; writable: boolean; sendable?: boolean; placeholder: string; label: string;
    onSend: (content: Inline[], clientId: string, expectedSeq: number | null) => Promise<Result>; accepts: (result: Result) => boolean; onAccepted?: (result: Result, clearIntent: boolean) => void;
    onCancel?: () => void; initial?: string; autofocus?: boolean; editVersion?: DocumentComment }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const controller = useRecovery();
  const draft = useCommentDrafts(controller);
  const id = commentSlotKey(target), local = draft.data.slots[id];
  const text = local?.text ?? initial, pending = draft.pending.has(id);
  const [error, setError] = useState<unknown>(null);
  const [review, setReview] = useState<DocumentComment | null>(null);
  const nativeConflict = staleEditedComment(error);
  const stale = target.kind === 'edit' && nativeConflict?.id === target.comment && nativeConflict.discussion_id === target.discussion ? nativeConflict : null;
  const needsReview = target.kind === 'edit' && (local?.baseSeq === null || local?.baseSeq === undefined || local.conflictSeq !== null);
  const canReview = target.kind === 'edit' && editVersion !== undefined;
  const reviewAction = canReview ? <Button size="sm" variant="ghost" disabled={!writable || pending} onClick={() => {
    setReview(stale !== null && stale.seq_updated >= editVersion.seq_updated ? stale : editVersion);
  }}>{t('documents.comments.reviewEdit')}</Button> : undefined;
  const root = useRef<HTMLDivElement>(null);
  const reader = useMemo(() => commentReader(), []);
  const alive = useRef(false);
  const source = useRef({ controller, id, docId, writable, sendable, onAccepted });
  const activeSource = () => alive.current ? source.current : null;
  useLayoutEffect(() => { source.current = { controller, id, docId, writable, sendable, onAccepted }; });
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useLayoutEffect(() => {
    const field = root.current?.querySelector('textarea');
    if (field !== null && field !== undefined) field.readOnly = !writable;
  }, [writable]);
  useLayoutEffect(() => {
    const selected = controller.store.getState().data.slots[id]?.selection;
    const field = root.current?.querySelector('textarea');
    if (selected !== null && selected !== undefined && field !== null && field !== undefined) {
      field.setSelectionRange(selected.start, selected.end, selected.direction);
    }
  }, [controller, id]);
  const send = async () => {
    const current = activeSource();
    if (current === null || !currentCommentReader(reader) || current.controller !== controller || current.id !== id
      || current.docId !== docId || controller.store.getState().data.document !== docId || !current.sendable || pending || needsReview || stale !== null || text.trim() === '') return;
    const native = documentStateNow(docId);
    if (native.ended !== null || native.document === null || !holds(native.document.access, 'doc.comment') || native.document.archived
      || native.document.inspection !== null && native.document.inspection !== undefined) return;
    const latest = controller.store.getState().data.slots[id];
    const words = latest?.text ?? text;
    const sent = beginCommentSend(controller, target, words, inlineOf(words.trim(), latest?.mentions ?? []));
    if (sent === null) return;
    setError(null);
    try {
      const result = await onSend(sent.attempt.content, sent.attempt.clientId, sent.attempt.expectedSeq);
      if (!accepts(result)) throw new NetworkError('malformed');
      const sameReader = currentCommentReader(sent.reader) && controller.generation === sent.generation;
      const cleared = finishCommentSend(controller, sent, true);
      const active = activeSource();
      const sameSource = active !== null && active.controller === controller && active.id === id && active.docId === docId;
      if (sameReader) (sameSource ? active.onAccepted : onAccepted)?.(result, cleared && sameSource);
      if (sameReader && target.kind !== 'edit') void queryClient.resetQueries({ queryKey: documentNoticeJobKey(docId) });
    } catch (failure) {
      finishCommentSend(controller, sent, false);
      const conflict = staleEditedComment(failure);
      if (target.kind === 'edit' && conflict?.id === target.comment && conflict.discussion_id === target.discussion) markCommentConflict(controller, sent, conflict.seq_updated);
      const active = activeSource();
      if (active !== null && currentCommentReader(sent.reader) && controller.generation === sent.generation
        && active.controller === controller && active.id === id && active.docId === docId) setError(failure);
    }
  };
  return (
    <div ref={root} className="flex flex-col gap-2" onSelectCapture={(event) => {
      const field = event.target;
      if (currentCommentReader(reader) && field instanceof HTMLTextAreaElement) selectCommentText(controller, target, {
        start: field.selectionStart, end: field.selectionEnd, direction: field.selectionDirection,
      });
    }}>
      <CommentField
        docId={docId}
        blockId={target.kind === 'new' ? target.block : blockId !== undefined ? blockId : documentStateNow(docId).discussions.get(target.discussion)?.block_id}
        label={label}
        value={text}
        placeholder={placeholder}
        autofocus={autofocus}
        onSend={() => void send()}
        onChange={(next, current) => {
          if (!writable || !currentCommentReader(reader)) return;
          setCommentText(controller, target, next, mentionedReaders(next, local?.mentions ?? [], current));
        }}
      />
      {error !== null && <ErrorAlert error={error} action={stale === null ? undefined : reviewAction} />}
      {needsReview && error === null && <InlineAlert tone="warning" action={reviewAction}>{t('documents.comments.editNeedsReview')}</InlineAlert>}
      {review !== null && <Dialog open onOpenChange={(open) => { if (!open) setReview(null); }} title={t('documents.comments.reviewEdit')}
        footer={<><Button variant="ghost" onClick={() => setReview(null)}>{t('common.cancel')}</Button>
          <Button disabled={!writable || pending} onClick={() => {
            if (target.kind === 'edit' && currentCommentReader(reader) && reviewCommentBase(controller, target, review.seq_updated)) { setError(null); setReview(null); }
          }}>{t('documents.comments.keepDraftOnVersion')}</Button></>}>
        <p className="text-sm font-medium text-text">{t('documents.comments.currentComment')}</p>
        <CommentText content={review.content} />
        <p className="text-sm font-medium text-text">{t('documents.comments.localDraft')}</p>
        <p className="text-sm break-words whitespace-pre-wrap text-text">{text}</p>
      </Dialog>}
      <div className="flex justify-end gap-2">
        {onCancel !== undefined && (
          <Button size="sm" variant="ghost" onClick={() => { if (discardComment(controller, target)) onCancel(); }}>
            {t('common.cancel')}
          </Button>
        )}
        <Button size="sm" variant="primary" icon={<SendHorizontal aria-hidden />} pending={pending} disabled={!sendable || needsReview || stale !== null || draft.forget.has(id) || text.trim() === ''} onClick={() => void send()}>
          {t('documents.comments.send')}
        </Button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- a discussion

function put(docId: string, discussion: DocumentDiscussion): void {
  documentWriter.apply(docId, { discussions: [discussion] }, discussion.seq_updated);
}

function withComment(discussion: DocumentDiscussion, comment: DocumentComment): DocumentDiscussion {
  // The caller below supplies the current discussion; never replace a newer
  // version of this comment with a late mutation answer.
  const known = discussion.comments.find((one) => one.id === comment.id);
  if (known !== undefined && known.seq_updated >= comment.seq_updated) return discussion;
  const has = discussion.comments.some((one) => one.id === comment.id);
  return {
    ...discussion,
    comments: (has ? discussion.comments.map((one) => (one.id === comment.id ? comment : one)) : [...discussion.comments, comment]).slice(-50),
    seq_updated: Math.max(discussion.seq_updated, comment.seq_updated),
  };
}

const CommentView = memo(function CommentView({ doc, discussion, comment, me, canComment }: { doc: Document; discussion: DocumentDiscussion; comment: DocumentComment; me: string | null; canComment: boolean }) {
  const { t } = useTranslation();
  const controller = useRecovery();
  const recovery = useCommentDrafts(controller);
  const editTarget: CommentTarget = { kind: 'edit', discussion: discussion.id, comment: comment.id };
  const editing = recovery.data.slots[commentSlotKey(editTarget)] !== undefined;
  const [error, setError] = useState<unknown>(null);
  const own = comment.author.id === me;
  const canManage = holds(doc.access, 'doc.manage');
  const react = async (emoji: string, on: boolean) => {
    try {
      const changed = await reactToComment(doc.id, comment.id, emoji, on);
      put(doc.id, withComment(documentStateNow(doc.id).discussions.get(discussion.id) ?? discussion, changed));
    } catch (failure) {
      setError(failure);
    }
  };
  if (comment.deleted && !editing) return <p className="text-sm text-text-subtle italic">{t('documents.comments.deleted')}</p>;
  return (
    <PersonThingProvider thing={{ kind: 'document', id: doc.id, ...(discussion.block_id === null ? {} : { anchor: { block: discussion.block_id } }) }}><li className="flex gap-2" data-comment={comment.id}>
      <div className="min-w-0 flex-1">
        <p className="flex flex-wrap items-baseline gap-x-2 text-xs text-text-muted">
          <UserChip user={comment.author} size={24} />
          <RelativeTime at={comment.created_at} />
          {comment.edited_at !== null && <span>{t('documents.comments.edited')}</span>}
        </p>
        {comment.deleted && <p className="text-sm text-text-subtle italic">{t('documents.comments.deleted')}</p>}
        {editing ? (
          <Composer
            docId={doc.id}
            target={editTarget}
            blockId={discussion.block_id}
            editVersion={comment}
            writable={canComment && own && !comment.deleted}
            autofocus
            label={t('documents.comments.edit')}
            placeholder=""
            initial={comment.content.map((run) => (run.type === 'text' ? run.text : run.type === 'mention' ? `@${run.user?.username ?? ''}` : run.type === 'break' ? '\n' : '')).join('')}
            onCancel={() => undefined}
            onSend={(content, clientId, expectedSeq) => expectedSeq === null ? Promise.reject(new NetworkError('malformed'))
              : editComment(doc.id, comment.id, content, { clientId, expectedSeq })}
            accepts={(edited) => edited.id === comment.id && edited.discussion_id === discussion.id}
            onAccepted={(edited) => put(doc.id, withComment(documentStateNow(doc.id).discussions.get(discussion.id) ?? discussion, edited))}
          />
        ) : (
          <CommentText content={comment.content} />
        )}
        {comment.reactions.length > 0 && (
          <ul className="mt-1 flex flex-wrap gap-1">
            {comment.reactions.map((reaction) => {
              const mine = me !== null && reaction.users.includes(me);
              return (
                <li key={reaction.emoji}>
                  <button
                    type="button"
                    aria-pressed={mine}
                    disabled={!canComment}
                    className={cx('rounded-full border px-2 py-1 text-xs', mine ? 'border-accent bg-accent-subtle text-text' : 'border-border text-text-muted hover:bg-bg-muted')}
                    onClick={() => void react(reaction.emoji, !mine)}
                  >
                    {reaction.emoji} {reaction.users.length}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
        {error !== null && <ErrorAlert error={error} />}
        {!editing && canComment && (
          <div className="mt-0.5 flex items-center gap-1">
            <ReactButton onPick={(emoji) => void react(emoji, true)} />
            {own && <IconButton size="sm" label={t('documents.comments.edit')} icon={<Pencil aria-hidden />} onClick={() => {
              const text = comment.content.map((run) => run.type === 'text' ? run.text : run.type === 'break' ? '\n' : run.type === 'mention' ? `@${run.user?.username ?? ''}` : '').join('');
              const mentions = comment.content.flatMap((run) => run.type === 'mention' && run.user !== null && run.user !== undefined ? [{ id: run.user.id, username: run.user.username }] : []);
              setCommentText(controller, editTarget, text, mentions, comment.seq_updated);
            }} />}
            {(own || canManage) && (
              <IconButton
                size="sm"
                label={t('documents.comments.delete')}
                icon={<Trash2 aria-hidden />}
                onClick={() => void deleteComment(doc.id, comment.id).catch(setError)}
              />
            )}
          </div>
        )}
      </div>
    </li></PersonThingProvider>
  );
});

function DiscussionCard({ doc, discussion, me, canComment, blockText }: { doc: Document; discussion: DocumentDiscussion; me: string | null; canComment: boolean; blockText: string | null }) {
  const { t } = useTranslation();
  const marks = useDocumentMarksStore();
  const [error, setError] = useState<unknown>(null);
  const controller = useRecovery();
  const recovery = useCommentDrafts(controller);
  const focused = recovery.data.focused === discussion.id;
  const replyTarget: CommentTarget = { kind: 'reply', discussion: discussion.id };
  const replying = recovery.data.slots[commentSlotKey(replyTarget)] !== undefined;
  const [commentCursor, setCommentCursor] = useState<string | null>(null);
  const pagedComments = discussion.comments.length >= 50 || commentCursor !== null;
  const commentPage = useQuery({ ...commentPageQuery(doc.id, discussion.id, commentCursor, discussion.seq_updated), enabled: pagedComments, placeholderData: (previous) => previous });
  const comments = commentPage.data?.items ?? discussion.comments;
  const wrote = commentPage.data?.wrote === true || comments.some((one) => one.author.id === me);
  const canResolve = canComment && (wrote || holds(doc.access, 'doc.edit'));
  const goTo = () => {
    commentIntent(controller, controller.store.getState().data.compose, discussion.id);
    marks.setState({ focused: discussion.id });
    if (discussion.block_id === null) return;
    scrollToDocumentBlock(discussion.block_id, marks);
    highlightBlock(discussion.block_id, marks);
  };
  const resolve = async (resolved: boolean) => {
    try {
      put(doc.id, await resolveDiscussion(doc.id, discussion.id, resolved));
    } catch (failure) {
      setError(failure);
    }
  };
  return (
    <section
      aria-label={t('documents.comments.discussion')}
      data-discussion-card={discussion.id}
      className={cx('flex flex-col gap-2 rounded-lg border bg-surface p-3', focused ? 'border-accent' : 'border-border', discussion.resolved !== null && 'opacity-80')}
    >
      <button type="button" className="text-left text-xs text-text-muted hover:text-text focus-visible:outline-2 focus-visible:outline-focus" onClick={goTo}>
        {discussion.quote !== '' ? <q className="italic">{discussion.quote}</q> : blockText !== null && blockText !== '' ? <span className="line-clamp-1">{blockText}</span> : <span>{t('documents.comments.wholeBlock')}</span>}
        {!discussion.anchored && discussion.block_id !== null && <span className="mt-0.5 block text-warning">{t('documents.comments.changed')}</span>}
      </button>
      {discussion.resolved !== null ? (
        <p className="text-xs text-text-muted"><Trans i18nKey="documents.comments.resolvedByChip" components={{ person: <UserChip user={discussion.resolved.by} size={24} thing={{ kind: 'document', id: doc.id, ...(discussion.block_id === null ? {} : { anchor: { block: discussion.block_id } }) }} /> }} /></p>
      ) : null}
      <ul className="flex flex-col gap-3">
        {comments.map((comment) => (
          <CommentView key={comment.id} doc={doc} discussion={discussion} comment={comment} me={me} canComment={canComment} />
        ))}
      </ul>
      {pagedComments && <div role="group" aria-label={t('people.commentHistory')} className="flex flex-wrap items-center gap-2">
        {commentCursor !== null && <Button size="sm" variant="ghost" disabled={commentPage.isFetching} onClick={() => setCommentCursor(null)}>{t('people.latestComments')}</Button>}
        {commentPage.data?.next_cursor !== null && commentPage.data?.next_cursor !== undefined && <Button size="sm" variant="ghost" disabled={commentPage.isFetching} onClick={() => setCommentCursor(commentPage.data?.next_cursor ?? null)}>{t('people.olderComments')}</Button>}
        {commentPage.isFetching && <span role="status" className="text-xs text-text-muted">{t('people.loadingComments')}</span>}
        {commentPage.isError && <ErrorAlert error={commentPage.error} action={<Button size="sm" onClick={() => void commentPage.refetch()}>{t('common.tryAgain')}</Button>} />}
      </div>}
      {error !== null && <ErrorAlert error={error} />}
      {(canComment && discussion.resolved === null || replying) && (
        <Composer
          docId={doc.id}
          target={replyTarget}
          blockId={discussion.block_id}
          writable={canComment}
          sendable={canComment && discussion.resolved === null}
          label={t('documents.comments.answer')}
          placeholder={t('documents.comments.answerPlaceholder')}
          onSend={(content, clientId) => answerDiscussion(doc.id, discussion.id, content, clientId)}
          accepts={(comment) => comment.discussion_id === discussion.id}
          onAccepted={(comment) => { setCommentCursor(null); put(doc.id, withComment(documentStateNow(doc.id).discussions.get(discussion.id) ?? discussion, comment)); }}
        />
      )}
      {canResolve && (
        <div className="flex justify-end">
          <Button size="sm" variant="ghost" icon={discussion.resolved === null ? <Check aria-hidden /> : <RotateCcw aria-hidden />} onClick={() => void resolve(discussion.resolved === null)}>
            {discussion.resolved === null ? t('documents.comments.resolve') : t('documents.comments.reopen')}
          </Button>
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- the panel

/** The Comments panel: every discussion beside the document, and where a new one is begun. It fills the Comments view of the right bar. */
export function CommentsPanel({ doc }: { doc: Document }) {
  const user = useStore(client.session, (state) => state.userId);
  const status = useStore(client.session, (state) => state.status);
  const version = useStore(client.session, (state) => state.readerVersion);
  const instance = useViewInstance();
  const scope = useDocumentMarksScope();
  const owner = instance?.id ?? scope?.owner ?? 'document';
  const controller = useMemo(() => getCommentDrafts(status === 'signed_in' ? user ?? '' : '', owner, doc.id), [user, status, owner, doc.id]);
  useCommentDraftGuard(instance?.id, controller);
  if (user === null || status !== 'signed_in') return null;
  return <PersonThingProvider thing={{ kind: 'document', id: doc.id }}><DraftContext.Provider value={controller}><CommentsBody key={JSON.stringify([user, version, owner, doc.id])} doc={doc} /></DraftContext.Provider></PersonThingProvider>;
}

function CommentsBody({ doc }: { doc: Document }) {
  const { t } = useTranslation();
  const controller = useRecovery();
  const recovery = useCommentDrafts(controller);
  const state = useDocumentState(doc.id);
  const marks = useDocumentMarksStore();
  const compose = recovery.data.compose;
  const reader = useMemo(() => commentReader(), []);
  const boundMarks = useRef<ReturnType<typeof useDocumentMarksStore> | null>(null);
  const reset = useRef(recovery.reset);
  useLayoutEffect(() => {
    const data = controller.store.getState().data;
    const changedDevice = reset.current !== recovery.reset;
    reset.current = recovery.reset;
    const current = marks.getState();
    const switchedOwner = boundMarks.current !== null && boundMarks.current !== marks;
    boundMarks.current = marks;
    if (changedDevice) {
      if (current.compose === commentComposeSource(controller)) marks.setState({ compose: null });
      rememberCommentComposeSource(controller, marks.getState().compose);
    } else if (!switchedOwner && current.compose !== null && current.compose !== commentComposeSource(controller)) {
      // A fresh gutter action can open another anchor; it never retargets an
      // older slot's body, which remains recoverable under its own target.
      rememberCommentComposeSource(controller, current.compose);
      commentIntent(controller, current.compose, current.focused);
    } else if (current.compose === null && data.compose !== null) {
      marks.setState({ compose: data.compose }); rememberCommentComposeSource(controller, data.compose);
    }
    return marks.subscribe((next, before) => {
      if (currentCommentReader(reader) && (next.compose !== before.compose || next.focused !== before.focused)) {
        if (next.compose !== before.compose) {
          rememberCommentComposeSource(controller, next.compose); commentIntent(controller, next.compose, next.focused);
        } else commentIntent(controller, controller.store.getState().data.compose, next.focused);
      }
    });
  }, [controller, marks, reader, recovery.reset]);
  const [show, changeShow] = useListChoice<Show>(`comments.${doc.id}`, 'show', 'open');
  const [order, changeOrder] = useListChoice<Order>(`comments.${doc.id}`, 'order', 'page');
  const [discussionCursor, setDiscussionCursor] = useState<string | null>(null);
  const pagedDiscussions = state.discussions.size >= 50 || discussionCursor !== null;
  const discussionPage = useQuery({ ...discussionPageQuery(doc.id, show, discussionCursor, state.lastSeq), enabled: pagedDiscussions, placeholderData: (previous) => previous });
  const setShow = (next: Show) => { setDiscussionCursor(null); changeShow(next); };
  const setOrder = (next: Order) => { changeOrder(next); };
  const me = client.session.getState().userId;
  const canComment = holds(doc.access, 'doc.comment') && !doc.archived && (doc.inspection === null || doc.inspection === undefined);

  const placeOf = useMemo(() => {
    const index = new Map(preOrder(buildTree(state.blocks.values())).map((id, at) => [id, at]));
    return (id: string | null) => (id === null ? Number.MAX_SAFE_INTEGER : (index.get(id) ?? Number.MAX_SAFE_INTEGER));
  }, [state.blocks]);
  const discussions = useMemo(() => {
    const all = (discussionPage.data?.items ?? [...state.discussions.values()]).filter((one) => Object.values(recovery.data.slots).some((draft) => draft.target.kind !== 'new' && draft.target.discussion === one.id)
      || (show === 'all' ? true : show === 'open' ? one.resolved === null : one.resolved !== null));
    return all.sort((one, other) => (order === 'page' ? placeOf(one.block_id) - placeOf(other.block_id) || one.created_at.localeCompare(other.created_at) : (other.comments.at(-1)?.created_at ?? other.created_at).localeCompare(one.comments.at(-1)?.created_at ?? one.created_at)));
  }, [state.discussions, show, order, placeOf, recovery.data.slots, discussionPage.data]);
  const here = discussions.filter((one) => one.block_id !== null);
  const gone = discussions.filter((one) => one.block_id === null);
  const textOf = (id: string | null) => (id === null ? null : plainText(state.blocks.get(id)?.content ?? []));
  const newTargets = Object.values(recovery.data.slots).flatMap((draft) => draft.target.kind === 'new' ? [draft.target] : []);
  if (compose !== null && !newTargets.some((target) => target.block === compose.block && target.quote === compose.quote)) newTargets.push({ kind: 'new', ...compose });

  return (
    <section aria-label={t('documents.comments.panel')} className="flex h-full min-h-0 flex-col gap-3 overflow-y-auto p-3">
      <h2 className="sr-only">{t('documents.page.comments')}</h2>
      <DocumentNoticeJobs document={doc.id} />
      <div className="flex gap-2">
        <Select<Show> label={t('documents.comments.show')} hideLabel value={show} onValueChange={setShow} options={[{ value: 'open', ...choiceOption(t, 'commentState.open') }, { value: 'resolved', ...choiceOption(t, 'commentState.resolved') }, { value: 'all', ...choiceOption(t, 'commentState.all') }]} />
        <Select<Order> label={t('documents.comments.order')} hideLabel value={order} onValueChange={setOrder} options={[{ value: 'page', ...choiceOption(t, 'commentOrder.page') }, { value: 'latest', ...choiceOption(t, 'commentOrder.latest') }]} />
      </div>
      {pagedDiscussions && <div role="group" aria-label={t('people.discussionHistory')} className="flex flex-wrap items-center gap-2">
        <p className="w-full text-sm text-text-muted">{t('people.discussionWindow')}</p>
        {discussionCursor !== null && <Button size="sm" variant="ghost" disabled={discussionPage.isFetching} onClick={() => setDiscussionCursor(null)}>{t('people.latestDiscussions')}</Button>}
        {discussionPage.data?.next_cursor !== null && discussionPage.data?.next_cursor !== undefined && <Button size="sm" variant="ghost" disabled={discussionPage.isFetching} onClick={() => setDiscussionCursor(discussionPage.data?.next_cursor ?? null)}>{t('people.olderDiscussions')}</Button>}
        {discussionPage.isFetching && <span role="status" className="text-xs text-text-muted">{t('people.loadingDiscussions')}</span>}
        {discussionPage.isError && <ErrorAlert error={discussionPage.error} action={<Button size="sm" onClick={() => void discussionPage.refetch()}>{t('common.tryAgain')}</Button>} />}
      </div>}
      {!recovery.saved && <ErrorAlert error={recovery.error} message={t('documents.comments.localSaveFailed')}
        action={<Button size="sm" variant="ghost" onClick={() => { saveCommentDrafts(controller); }}>{t('common.tryAgain')}</Button>} />}
      {canComment && compose === null && <Button size="sm" icon={<MessageSquarePlus aria-hidden />} onClick={() => {
        const first = preOrder(buildTree(state.blocks.values()))[0];
        if (first !== undefined) marks.setState({ compose: { block: first, quote: '' } });
      }} disabled={state.blocks.size === 0}>{t('people.startDiscussion')}</Button>}
      {canComment && compose === null && state.blocks.size === 0 && <p className="text-xs text-text-muted">{t('people.commentNeedsText')}</p>}
      {newTargets.map((target) => (
        <section key={commentSlotKey(target)} aria-label={t('documents.comments.new')} className="flex flex-col gap-2 rounded-lg border border-accent bg-surface p-3">
          {target.quote !== '' && <q className="text-xs text-text-muted italic">{target.quote}</q>}
          {(!state.blocks.has(target.block) || state.blocks.get(target.block)?.removed === true) && <p className="text-xs text-text-muted">{t('documents.comments.removed')}</p>}
          <Composer
            docId={doc.id}
            target={target}
            writable={canComment}
            sendable={canComment && state.blocks.has(target.block) && state.blocks.get(target.block)?.removed !== true}
            autofocus={compose?.block === target.block && compose.quote === target.quote}
            label={t('documents.comments.new')}
            placeholder={t('documents.comments.newPlaceholder')}
            onCancel={() => { if (marks.getState().compose?.block === target.block && marks.getState().compose?.quote === target.quote) marks.setState({ compose: null }); }}
            onSend={(content, clientId) => startDiscussion(doc.id, { block_id: target.block, ...(target.quote === '' ? {} : { quote: target.quote }), content, client_id: clientId })}
            accepts={(made) => made.document_id === doc.id && (made.block_id === null || made.block_id === target.block)}
            onAccepted={(made, clearIntent) => {
              put(doc.id, made);
              if (clearIntent) {
                commentIntent(controller, controller.store.getState().data.compose, made.id);
                const current = marks.getState().compose;
                if (current?.block === target.block && current.quote === target.quote) marks.setState({ compose: null, focused: made.id });
              }
            }}
          />
        </section>
      ))}
      {discussions.length === 0 && newTargets.length === 0 && (
        <p className="flex flex-col items-center gap-2 py-6 text-center text-sm text-text-muted">
          <MessageSquare aria-hidden className="size-4" />
          {t(show === 'resolved' ? 'documents.comments.noneResolved' : 'documents.comments.none')}
        </p>
      )}
      {here.map((one) => (
        <DiscussionCard key={one.id} doc={doc} discussion={one} me={me} canComment={canComment} blockText={textOf(one.block_id)} />
      ))}
      {gone.length > 0 && (
        <>
          <h3 className="mt-2 text-sm font-semibold text-text-muted">{t('documents.comments.removed')}</h3>
          {gone.map((one) => (
            <DiscussionCard key={one.id} doc={doc} discussion={one} me={me} canComment={canComment} blockText={null} />
          ))}
        </>
      )}
    </section>
  );
}

export { ApiError };
