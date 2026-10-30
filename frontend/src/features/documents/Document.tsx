/** A document's page: its title, its description, the line under it with who changed it last and its access in words, who is here, its blocks, Comments, History, and its menu (Private, Move, Share with an agent, Export, Print, Delete).
 *
 * One view, and it is the document: whoever may write in it clicks and types. There is no Save and no
 * preview beside it; the header says "Saving…" and then "Saved". Somebody who may only read sees the
 * same page without a caret and is told so in one line.
 */
import { useViewSaveGuard } from '@/features/workbench/CloseViews';
import { useViewInstance } from '@/features/workbench/instance';
import { useWorkbench } from '@/features/workbench/store';
import { useQuery } from '@tanstack/react-query';
import { useBlocker } from '@tanstack/react-router';
import { Clock, FileText, History as HistoryIcon, MoreHorizontal, Search, Share2 } from 'lucide-react';
import { type RefObject, useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { AdminReachNotice } from '@/features/access/AdminReachNotice';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { describeDocument, markSeen, renameDocument, versionQuery } from '@/api/documents';
import { documentLinksQuery } from '@/api/links';
import { nodeQuery } from '@/api/nodes';
import { openAccess } from '@/features/access/AccessDialog';
import { copyText } from '@/ui/clipboard';
import { useCommentAnchors } from '@/features/documents/Comments';
import { BlockEditor, type EditorApi } from '@/features/documents/Editor';
import { printPage } from '@/features/documents/pdf';
import { openExport, requestExportAccess } from '@/features/export/ExportHost';
import { exportEntries } from '@/features/export/menu';
import { useTreeEntry } from '@/features/documents/useTreeEntry';
import { documentMetadataRefused, documentWithPlacement } from '@/features/documents/placementView';
import { useViewShown } from '@/features/workbench/viewOpening';
import { DocumentLocation } from '@/features/documents/DocumentLocation';
import { FilesDialog } from '@/features/documents/Files';
import { HistoryDrawer, useHistoryDot } from '@/features/documents/History';
import { highlightBlock, revealDocumentBlock, useDocumentMarksScope, useDocumentMarksStore } from '@/features/documents/marks';
import { PrivateDialog } from '@/features/documents/Placement';
import { LinksDialog } from '@/features/links/LinksDialog';
import { SinceYouLooked } from '@/features/documents/SinceYouLooked';
import type { SavePhase } from '@/features/documents/sync';
import { revealComments } from '@/features/documents/views';
import { DocumentOrganize } from '@/features/documents/DocumentOrganize';
import { DiscussionBell } from '@/features/people/DiscussionBell';
import { PersonThingProvider } from '@/features/people/personContext';
import { ReaderComment } from '@/features/people/ReaderComment';
import { useViewFind } from '@/features/search/ViewFind';
import { useShape } from '@/features/workbench/Narrow';
import { useReadmeEntry } from '@/features/documents/Readme';
import type { Document, Node, SearchHit, UserRef } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { reloadDocument, useDocumentFeed, useOrgTreeFeed } from '@/realtime/live';
import { useDocumentState } from '@/realtime/stores/document';
import { useKeyBinding } from '@/shell/keys';
import { useScreenHeading } from '@/shell/screen';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { actionName } from '@/ui/actions';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { LockBadge } from '@/ui/LockBadge';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { PresenceStack } from '@/ui/PresenceStack';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import { AuthorChip } from '@/ui/AuthorChip';

// The Comments view of the right bar is this part's: it takes the place of the note the workbench shows until then.

/** How long a title, or a description, rests after a key before it is sent. */
const RENAME_AFTER_MS = 700;
/** The most characters of a description: what the server keeps. */
const DESCRIPTION_MAX_CHARS = 4_000;
/** How long a document must have been on screen before it counts as having been looked at. */
const SEEN_AFTER_MS = 3_000;

export interface DocumentPageProps {
  documentId: string;
  /** A second document view keeps the route's main heading and address. */
  embedded?: boolean;
  /** The document was just made, and named where it was made: it opens with the cursor at the start of its text. */
  fresh?: boolean | undefined;
  /** The cursor was put in the text: the address says no more that the document is new, so that a reload does not do it again. */
  onOffered?: (() => void) | undefined;
  /** A block to open at, from a link, a search hit or a discussion. */
  block?: string | undefined;
  /** An attached file to open, optionally at a text line. */
  file?: string | undefined;
  line?: number | undefined;
  onFileClosed?: (() => void) | undefined;
  /** A version to show instead of the present one. */
  version?: number | undefined;
  onVersion?: (version: number | undefined) => void;
}

/** The words for who may do what with a document: its access, said in one line. */
type Say = TFunction;

function accessWords(t: Say, doc: Document): { text: string; lock: boolean } {
  if (doc.page === 'restricted') return { text: doc.owners > 1 ? t('documents.access.privateOwners') : t('documents.access.private'), lock: true };
  if (doc.page === 'custom') return { text: t('documents.access.custom'), lock: false };
  const place = doc.node.name ?? t('documents.access.place');
  return { text: t('documents.access.follows', { place }), lock: false };
}

function madeBy(t: Say, doc: Document): string | null {
  const by = doc.created_by;
  if ('agent' in by) return t('documents.access.madeBy', { agent: by.agent, thread: by.thread.name ?? '' });
  return null;
}

export function DocumentPage({ documentId, block, file, line, onFileClosed, version, fresh = false, onOffered, onVersion, embedded = false }: DocumentPageProps) {
  const { t } = useTranslation();
  const instance = useViewInstance();
  const marks = useDocumentMarksStore();
  const marksScope = useDocumentMarksScope();
  useDocumentFeed(documentId);
  const state = useDocumentState(documentId);
  const shown = useViewShown();
  // The addressed frame owns the single metadata feed. An embedded page has no such owner.
  const metadata = useTreeEntry({ kind: 'document', id: documentId }, false, embedded && shown && state.document !== null && !state.document.deleted);
  const doc = !documentMetadataRefused(state.error) && metadata.isSuccess ? documentWithPlacement(state.document, metadata.data) : null;
  const visitId = doc !== null && !doc.deleted && state.ended === null ? doc.id : undefined;
  const place = useQuery({ ...nodeQuery(doc?.node.id ?? ''), enabled: doc !== null && !doc.node.redacted }).data;
  // A document that switches every tab of its Links section off does not offer the section.
  const tabs = useQuery({ ...documentLinksQuery(documentId), enabled: doc !== null && !doc.deleted && state.ended === null });
  // The places of the document's org: where it can be moved to, and what a link in its text can name.
  useOrgTreeFeed(doc?.path[0]?.redacted === false ? doc.path[0].id : null);
  const [editor, setEditor] = useState<EditorApi | null>(null);
  const enterBody = useRef(false);
  const editorReady = useCallback((api: EditorApi | null) => {
    setEditor(api);
    if (api !== null && enterBody.current) { enterBody.current = false; api.editor.commands.focus('start'); }
  }, []);
  const root = useRef<HTMLDivElement>(null);
  const findHit = useCallback((hit: SearchHit) => {
    if (hit.kind !== 'document' || hit.document.id !== documentId) return;
    const target = hit.block_id === null ? root.current?.querySelector<HTMLElement>('article textarea') : root.current?.querySelector<HTMLElement>(`[data-id="${CSS.escape(hit.block_id)}"]`);
    target?.scrollIntoView({ block: 'center' });
    if (hit.block_id !== null) highlightBlock(hit.block_id, marks);
  }, [documentId, marks]);
  const find = useViewFind({ root, filters: { document_id: documentId, kinds: ['document'] }, label: t('search.find.document'), onHit: findHit, content: 'article', enabled: doc !== null && !doc.deleted && state.ended === null && version === undefined });
  const [history, setHistory] = useState(false);
  const [dialog, setDialog] = useState<null | 'move' | 'private' | 'delete' | 'files' | 'duplicate' | 'links'>(null);
  const me = useStore(client.session, (session) => session.userId);
  // Beginning a discussion, from the words selected or from a block's menu, brings the Comments view to the front: it is where it is written.
  const beside = useShape() === 'wide';
  useEffect(
    () =>
      marks.subscribe((marks, before) => {
        if (marks.compose !== null && before.compose === null) revealComments(beside);
      }),
    [beside, marks],
  );

  useScreenHeading({ target: visitId === undefined ? undefined : { kind: 'document', id: visitId }, title: doc !== null && state.ended === null ? doc.title : state.ended !== null || state.error !== null || metadata.isError ? t('craft.documentUnavailable') : t('craftLoading.document'), icon: FileText, nodeId: state.ended === null ? doc?.node.id : undefined, path: state.ended === null ? doc?.path : undefined, leaf: true, views: { ready: doc !== null && state.ended === null, chat: place !== undefined && holds(place.access, 'chat.view'), links: tabs.data === undefined || tabs.data.shown.length > 0 } }, !embedded);
  // The words of each open discussion are marked in the page, whether or not the panel is open.
  useCommentAnchors(documentId);

  // The page is looked at once it has been on screen for a moment.
  useEffect(() => {
    if (doc === null || doc.deleted || version !== undefined) return;
    const timer = setTimeout(() => void markSeen(doc.id, doc.seq).catch(() => undefined), SEEN_AFTER_MS);
    return () => clearTimeout(timer);
    // Only the first whole load starts the wait: a document that others are writing in is not "seen" again at every row.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc?.id, version]);

  // Scoped marks belong to the view and survive a narrow switch to Comments. The unscoped
  // fallback is shared, so it must still be cleared when this page goes away.
  useEffect(() => () => { if (marksScope === null) marks.setState({ compose: null, focused: null }); }, [documentId, marks, marksScope]);

  // A link to a block opens at that block, marked for a moment.
  useEffect(() => {
    if (block === undefined || editor === null) return;
    let current = true;
    revealDocumentBlock(block, marks, () => current, editor.editor);
    return () => { current = false; };
  }, [block, editor, marks]);

  const blocker = useBlocker({
    shouldBlockFn: () => instance === null && editor?.sync?.hasUnsent() === true,
    enableBeforeUnload: () => editor?.sync?.hasUnsent() === true,
    withResolver: true,
  });

  useEffect(() => {
    const sync = editor?.sync, id = instance?.id;
    if (sync === null || sync === undefined || id === undefined) return;
    const update = () => useWorkbench.getState().setUnsaved(id, sync.hasUnsent());
    update();
    return sync.save.subscribe(update);
  }, [editor, instance?.id]);
  useViewSaveGuard(instance?.id, {
    save: () => new Promise<boolean>((resolve) => {
      const sync = editor?.sync;
      if (sync === null || sync === undefined) { resolve(false); return; }
      if (!sync.hasUnsent()) { resolve(true); return; }
      const stop = sync.save.subscribe((state) => {
        if (state.phase === 'saved') { stop(); resolve(true); }
        else if (state.phase === 'failed' || state.phase === 'offline') { stop(); resolve(false); }
      });
      sync.flush();
      const phase = sync.save.getState().phase;
      if (phase === 'saved' || phase === 'failed' || phase === 'offline') { stop(); resolve(phase === 'saved'); }
    }),
    discard: () => { editor?.sync?.destroy(); },
  });

  // Ctrl+S (⌘S) sends what is unsent now, wherever the focus is on the page, and says how it went. A page that cannot
  // be written lets the key go on.
  const saveNow = useCallback((event: KeyboardEvent) => {
    if (!(event.target instanceof globalThis.Node) || root.current?.contains(event.target) !== true) return false;
    const sync = editor?.sync;
    if (sync === null || sync === undefined) return false;
    sync.flush();
    const tell = (state: { phase: SavePhase }): boolean => {
      if (state.phase === 'saved') toast({ tone: 'success', title: t('documents.page.saved') });
      else if (state.phase === 'offline') toast({ tone: 'info', title: t('documents.page.offline') });
      else if (state.phase === 'failed') toast({ tone: 'danger', title: t('documents.page.failed') });
      else return false;
      return true;
    };
    if (!tell(sync.save.getState())) {
      const stop = sync.save.subscribe((state) => {
        if (tell(state)) stop();
      });
    }
    return true;
  }, [editor, t]);
  useKeyBinding('document-save', 'Mod+S', saveNow, 'documents.keys.save');

  if (metadata.isError) return <ErrorState error={metadata.error} onRetry={() => void metadata.retry()} />;
  if (state.ended !== null) {
    return <EmptyState icon={<FileText aria-hidden />} title={t(state.ended === 'deleted' ? 'documents.page.endedDeleted' : 'documents.page.endedRevoked')} />;
  }
  if (doc === null) {
    if (state.error !== null) return <ErrorState error={state.error} onRetry={() => reloadDocument(documentId)} />;
    return (
      <SkeletonGroup className="mx-auto w-full max-w-(--doc-column) gap-4 px-6 py-6">
        <Skeleton className="h-10 w-2/3" />
        <Skeleton className="h-4 w-1/3" />
        <Skeleton className="h-5 w-full" />
        <Skeleton className="h-5 w-5/6" />
        <Skeleton className="h-5 w-4/6" />
      </SkeletonGroup>
    );
  }

  const canEdit = holds(doc.access, 'doc.edit');
  const canComment = holds(doc.access, 'doc.comment');
  const editable = canEdit && !doc.archived && !doc.deleted && version === undefined;
  const owner = doc.owner;

  return (
    <PersonThingProvider thing={{ kind: 'document', id: doc.id }}><div ref={root} tabIndex={-1} className="flex h-full min-h-0 flex-col" data-screen="document">
      <PageBar doc={doc} editor={editable ? editor : null} onHistory={() => setHistory(true)} onFind={version === undefined && !doc.deleted ? find.show : null} onDialog={setDialog} me={me} place={place} />
      {find.bar}
      <div className="flex min-h-0 flex-1">
        <div className="min-w-0 flex-1 overflow-y-auto" data-document-scroll="">
          <article data-document={doc.id} className="mx-auto w-full max-w-(--doc-column) px-4 py-6 sm:pr-(--doc-gutter-end) sm:pl-(--doc-gutter-start)">
            <DocumentLocation metadata={metadata.data} error={metadata.error} onRetry={() => void metadata.retry()} />
            <TitleField key={`title-${doc.id}`} doc={doc} editable={editable} fresh={fresh} onOffered={onOffered}
              onEnter={() => { if (editor !== null) editor.editor.commands.focus('start'); else enterBody.current = true; }} />
            <DescriptionField key={`description-${doc.id}`} doc={doc} editable={editable}
              onEnter={() => { if (editor !== null) editor.editor.commands.focus('start'); else enterBody.current = true; }} />
            <LineUnderTitle doc={doc} />
            <Notices doc={doc} canEdit={canEdit} owner={owner} version={version} onVersion={onVersion} />
            {version === undefined && <SinceYouLooked key={`since-${doc.id}`} doc={doc} />}
            {!editable && version === undefined && !doc.archived && !doc.deleted && canComment && <ReaderComment docId={doc.id} />}
            {version === undefined ? (
              <BlockEditor key={`editor-${doc.id}`} document={doc} editable={editable} onReady={editorReady} />
            ) : (
              <VersionBody key={`version-${doc.id}-${version}`} doc={doc} version={version} onReady={editorReady} />
            )}
          </article>
        </div>
      </div>
      <HistoryDrawer
        doc={doc}
        open={history}
        onOpenChange={setHistory}
        onShow={(shown) => {
          onVersion?.(shown);
          setHistory(false);
        }}
      />
      <PrivateDialog doc={doc} open={dialog === 'private'} onOpenChange={(open) => !open && setDialog(null)} />
      <LinksDialog documentId={doc.id} open={dialog === 'links'} onOpenChange={(open) => !open && setDialog(null)} />
      <FilesDialog doc={doc} open={dialog === 'files' || file !== undefined} fileId={file} line={line} onOpenChange={(open) => { if (!open) { setDialog(null); onFileClosed?.(); } }} />
      {(dialog === 'move' || dialog === 'delete' || dialog === 'duplicate') && <DocumentOrganize id={doc.id} command={dialog === 'duplicate' ? 'copy' : dialog} onClose={() => setDialog(null)} />}
      <ConfirmDialog
        open={blocker.status === 'blocked'}
        onOpenChange={(open) => !open && blocker.reset?.()}
        title={t('documents.leave.title')}
        confirmLabel={t('documents.leave.confirm')}
        danger
        onConfirm={() => blocker.proceed?.()}
      >
        {t('documents.leave.about')}
      </ConfirmDialog>
    </div></PersonThingProvider>
  );
}

/** The document as it stood at a version: its blocks as the server kept them, read and not written. */
function VersionBody({ doc, version, onReady }: { doc: Document; version: number; onReady: (api: EditorApi | null) => void }) {
  const read = useQuery(versionQuery(doc.id, version));
  if (read.isPending) {
    return (
      <SkeletonGroup className="gap-3">
        <Skeleton className="h-5 w-full" />
        <Skeleton className="h-5 w-5/6" />
        <Skeleton className="h-5 w-4/6" />
      </SkeletonGroup>
    );
  }
  if (read.isError) return <ErrorState compact error={read.error} onRetry={() => void read.refetch()} />;
  return <BlockEditor document={doc} editable={false} fixed={read.data.blocks} onReady={onReady} />;
}

// ---------------------------------------------------------------- the bar

/** Share: the permission page of the document, with the field that adds people in focus, the one every other thing is shared from. */
const share = (doc: Document): void => openAccess({ kind: 'document', id: doc.id, name: doc.title });

interface PageBarProps {
  doc: Document;
  editor: EditorApi | null;
  onHistory: () => void;
  onFind: (() => void) | null;
  onDialog: (dialog: 'move' | 'private' | 'delete' | 'files' | 'duplicate' | 'links') => void;
  me: string | null;
  /** The place the document is written beside, as the reader sees it: whose README it can be. */
  place: Node | undefined;
}

function PageBar({ doc, editor, onHistory, onFind, onDialog, me, place }: PageBarProps) {
  const { t } = useTranslation();
  const state = useDocumentState(doc.id);
  const dot = useHistoryDot(doc);
  const canManage = holds(doc.access, 'doc.manage');
  const canShare = holds(doc.access, 'access.manage');
  const readme = useReadmeEntry(doc, place);
  const entries: MenuEntry[] = [
    // The bar's icon acts, by name, for keyboard and touch.
    ...(onFind === null ? [] : [{ id: 'find', label: t('search.find.document'), icon: <Search aria-hidden />, onSelect: onFind }]),
    { id: 'history', label: t('documents.page.history'), icon: <HistoryIcon aria-hidden />, onSelect: onHistory },
    { type: 'separator', id: 'sep-0' },
    { id: 'files', label: t('documents.files.menu'), onSelect: () => onDialog('files') },
    { id: 'duplicate', label: t('people.duplicate'), onSelect: () => onDialog('duplicate') },
    { type: 'separator', id: 'sep-1' },
    doc.page === 'restricted'
      ? { id: 'open', action: 'inheritAccess', label: t('documents.menu.openAgain'), disabled: canShare ? false : t('documents.menu.needsManage'), onSelect: () => onDialog('private') }
      : { id: 'private', action: 'makePrivate', label: t('documents.menu.private'), disabled: canShare ? false : t('documents.menu.needsManage'), onSelect: () => onDialog('private') },
    { id: 'move', label: t('documents.menu.move'), disabled: canManage ? false : t('documents.menu.needsManage'), onSelect: () => onDialog('move') },
    { id: 'links', label: t('links.menu'), disabled: canManage ? false : t('documents.menu.needsManage'), onSelect: () => onDialog('links') },
    ...(readme === null ? [] : [readme]),
    ...exportEntries({
      allowed: holds(doc.access, 'doc.export'),
      owner: doc.owner,
      where: 'screen',
      onExport: () => openExport({ kind: 'document', id: doc.id, name: doc.title }),
      onRequest: () => requestExportAccess({ kind: 'document', id: doc.id, name: doc.title, owner: doc.owner }),
      t,
    }),
    { id: 'print', label: t('documents.menu.print'), onSelect: () => printPage() },
    {
      id: 'copy',
      label: t('documents.menu.copyLink'),
      onSelect: () => void copyText(`${location.origin}/doc/${doc.id}`, t('common.linkCopied')),
    },
    { type: 'separator', id: 'sep-2' },
    { id: 'delete', label: t('documents.menu.delete'), danger: true, disabled: canManage ? false : t('documents.menu.needsManage'), onSelect: () => onDialog('delete') },
  ];
  return (
    <div className="flex min-h-12 shrink-0 flex-wrap items-center justify-end gap-x-2 gap-y-1 border-b border-border px-4 py-1">
      <AdminReachNotice of={doc.access.admin_access?.place} private={doc.page === 'restricted'} object={{ kind: 'document', id: doc.id }} />
      <PresenceStack users={state.presence} without={me} />
      {!doc.deleted && <DiscussionBell doc={doc} />}
      {onFind !== null && <IconButton label={t('search.find.document')} icon={<Search aria-hidden />} onClick={onFind} />}
      <SaveState editor={editor} />
      <span className="relative">
        <IconButton label={t('documents.page.history')} icon={<HistoryIcon aria-hidden />} onClick={onHistory} />
        {dot && <span aria-hidden className="absolute top-1 right-1 size-2 rounded-full bg-accent" data-testid="history-dot" />}
      </span>
      <Button size="sm" icon={<Share2 aria-hidden />} onClick={() => share(doc)}>
        {t('documents.page.share')}
      </Button>
      <Menu label={actionName('actions', doc.title)} side="bottom" align="end" trigger={<IconButton label={actionName('actions', doc.title)} icon={<MoreHorizontal aria-hidden />} />} entries={entries} />
    </div>
  );
}

/** "Saved", "Saving…", or "Not saved yet · reconnecting": what has become of what was typed. */
function SaveState({ editor }: { editor: EditorApi | null }) {
  const { t } = useTranslation();
  const sync = editor?.sync ?? null;
  const save = useStore(sync?.save ?? fallbackSave);
  if (sync === null) return null;
  const words = save.phase === 'saved' ? t('documents.page.saved') : save.phase === 'saving' ? t('documents.page.saving') : save.phase === 'offline' ? t('documents.page.offline') : t('documents.page.failed');
  return (
    <span role="status" aria-live="polite" data-save={save.phase} className={cx('flex items-center gap-1 text-sm', save.phase === 'failed' ? 'text-danger-text' : 'text-text-muted')}>
      <Clock aria-hidden className="size-4" />
      {words}
    </span>
  );
}

import { createStore } from 'zustand/vanilla';
const fallbackSave = createStore(() => ({ phase: 'saved' as const, error: null }));

// ---------------------------------------------------------------- title and the line under it

export function TitleField({ doc, editable, fresh = false, onOffered, onEnter }: { doc: Pick<Document, 'id' | 'title'>; editable: boolean; fresh?: boolean; onOffered?: (() => void) | undefined; onEnter: () => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState(doc.title);
  const sent = useRef(doc.title);
  const field = useFittedHeight(text);
  const offered = useRef(onOffered);
  const focused = useRef(false);
  useEffect(() => { offered.current = onOffered; }, [onOffered]);
  useEffect(() => {
    if (!fresh || !editable || focused.current || field.current === null) return;
    focused.current = true;
    field.current.focus();
    field.current.select();
    offered.current?.();
  }, [fresh, editable, field]);
  // A title that others changed arrives; one the person is typing stays.
  useEffect(() => {
    if (document.activeElement !== field.current && doc.title !== sent.current) {
      sent.current = doc.title;
      setText(doc.title);
    }
  }, [doc.title, field]);
  useEffect(() => {
    if (!editable || text.trim() === '' || text === sent.current) return;
    const timer = setTimeout(() => {
      sent.current = text;
      void renameDocument(doc.id, text.trim()).catch(() => {
        sent.current = doc.title;
      });
    }, RENAME_AFTER_MS);
    return () => clearTimeout(timer);
  }, [text, editable, doc.id, doc.title]);
  return (
    <textarea
      ref={field}
      data-content-role="document"
      rows={1}
      value={text}
      readOnly={!editable}
      aria-label={t('documents.page.titleLabel')}
      placeholder={t('documents.page.untitled')}
      maxLength={200}
      className="-mx-3 mb-1 block w-[calc(100%+1.5rem)] resize-none overflow-hidden rounded-none border-x-0 border-t-0 border-b border-control-border bg-transparent px-3 py-1 text-3xl leading-9 font-semibold text-text placeholder:text-text-subtle"
      onChange={(event) => setText(event.target.value.replace(/\n/g, ' '))}
      onKeyDown={(event) => {
        if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onEnter();
        }
      }}
    />
  );
}

/** A field as tall as its text: it grows when the text does, and when its column narrows without the text changing.
 * It gives the reference the field is drawn with. */
function useFittedHeight(text: string): RefObject<HTMLTextAreaElement | null> {
  const field = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const element = field.current;
    if (element === null) return;
    let live = true;
    const fit = () => {
      if (!live) return;
      element.style.height = '0px';
      element.style.height = `${element.scrollHeight}px`;
    };
    fit();
    let width = element.clientWidth;
    const observer = new ResizeObserver(() => {
      // A height update must not start another measurement loop. The field also grows when
      // the reader narrows its main group by resizing the inspector, without changing text.
      if (element.clientWidth === width) return;
      width = element.clientWidth;
      fit();
    });
    observer.observe(element);
    const fonts = document.fonts;
    void fonts.ready.then(fit);
    fonts.addEventListener('loadingdone', fit);
    return () => {
      live = false;
      observer.disconnect();
      fonts.removeEventListener('loadingdone', fit);
    };
  }, [text]);
  return field;
}

/** What the document is about, under its title: written in place by whoever may edit it and sent as it rests, as the
 * title is; Enter goes on to the text, Shift+Enter starts another line. Its first line is what lists show beside the
 * title. A reader sees it as written, and nothing where there is none. */
export function DescriptionField({ doc, editable, onEnter }: { doc: Pick<Document, 'id' | 'description'>; editable: boolean; onEnter: () => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState(doc.description);
  const sent = useRef(doc.description);
  const field = useFittedHeight(text);
  // A description that others changed arrives; one the person is typing stays.
  useEffect(() => {
    if (document.activeElement !== field.current && doc.description !== sent.current) {
      sent.current = doc.description;
      setText(doc.description);
    }
  }, [doc.description, field]);
  useEffect(() => {
    if (!editable || text === sent.current) return;
    const timer = setTimeout(() => {
      sent.current = text;
      void describeDocument(doc.id, text).catch(() => {
        sent.current = doc.description;
      });
    }, RENAME_AFTER_MS);
    return () => clearTimeout(timer);
  }, [text, editable, doc.id, doc.description]);
  if (!editable) {
    return doc.description.trim() === '' ? null : <p data-content-role="document" data-document-description className="mb-2 text-md whitespace-pre-line text-text-muted wrap-anywhere">{doc.description}</p>;
  }
  return (
    <textarea
      ref={field}
      data-content-role="document"
      rows={1}
      value={text}
      aria-label={t('documents.page.descriptionLabel')}
      placeholder={t('documents.page.descriptionPlaceholder')}
      maxLength={DESCRIPTION_MAX_CHARS}
      data-document-description
      className="-mx-3 mb-2 block w-[calc(100%+1.5rem)] resize-none overflow-hidden rounded-none border-x-0 border-t-0 border-b border-control-border bg-transparent px-3 py-0.5 text-md text-text-muted placeholder:text-text-subtle"
      onChange={(event) => setText(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onEnter();
        }
      }}
    />
  );
}

function LineUnderTitle({ doc }: { doc: Document }) {
  const { t } = useTranslation();
  const words = accessWords(t, doc);
  const agent = madeBy(t, doc);
  return (
    <p className="mb-6 flex flex-wrap items-center gap-x-2 text-sm text-text-muted">
      {doc.updated_by !== null && (
        <>
          <span className="inline-flex items-center gap-1">{t('agentIdentity.changedBy')} <AuthorChip author={doc.updated_by} /></span>
          <span><RelativeTime at={doc.updated_at} /></span>
          <span aria-hidden>·</span>
        </>
      )}
      {words.lock && <LockBadge label={words.text} />}
      <span data-access-state>{words.text}</span>
      {agent !== null && (
        <>
          <span aria-hidden>·</span>
          <span>{agent}</span>
        </>
      )}
    </p>
  );
}

function Notices({ doc, canEdit, owner, version, onVersion }: { doc: Document; canEdit: boolean; owner: UserRef; version: number | undefined; onVersion: ((version: number | undefined) => void) | undefined }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2 empty:hidden">
      {doc.deleted && <Banner tone="danger">{t('documents.page.deleted')}</Banner>}
      {doc.archived && <p className="text-sm text-text-muted">{t('documents.page.archived')}</p>}
      {!canEdit && !doc.archived && <p className="text-sm text-text-muted">{t('documents.page.readOnly', { name: owner.display_name })}</p>}
      {version !== undefined && (
        <Banner tone="info" action={<Button size="sm" onClick={() => onVersion?.(undefined)}>{t('documents.history.showPresent')}</Button>}>
          {t('documents.history.showingVersion', { version })}
        </Banner>
      )}
    </div>
  );
}
