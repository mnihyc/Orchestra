/** Real inspector bodies and the shared native document follower; only native reads/socket delivery are synthetic. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { useRef } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import type * as Router from '@tanstack/react-router';
import { ApiError, client } from '@/api/client';
import { commentDraftGuard, commentSlotKey, forgetCommentDraftDeviceData, getCommentDrafts, retireCommentDraftSession, setCommentText } from '@/features/documents/commentDrafts';
import { DocumentMarksProvider, retireDocumentMarks } from '@/features/documents/marks';
import '@/features/documents/register';
import { DocumentOutline } from '@/features/people/DocumentOutline';
import { ViewInstanceContext } from '@/features/workbench/instance';
import { findTab, makeGroup } from '@/features/workbench/layout';
import { panelTarget, setPanelScope } from '@/features/workbench/panelScope';
import { useWorkbench } from '@/features/workbench/store';
import { viewOf } from '@/features/workbench/views';
import type { Document, DocumentBlock, DocumentDiscussion, DocumentSync } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { feeds, socket, useDocumentFeed } from '@/realtime/live';
import type { FeedListener } from '@/realtime/socket';
import { documentStateNow, documentWriter } from '@/realtime/stores/document';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<typeof Documents.fetchDocumentSync>(), start: vi.fn<typeof Documents.startDiscussion>(),
  reply: vi.fn<typeof Documents.answerDiscussion>(), edit: vi.fn<typeof Documents.editComment>() }));
vi.mock('@/api/documents', async (load) => ({ ...await load<typeof Documents>(), fetchDocumentSync: calls.read,
  startDiscussion: calls.start, answerDiscussion: calls.reply, editComment: calls.edit }));
// An addressed inspector must never fall back to the unrelated browser route's document.
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useParams: () => ({ documentId: 'route-only-document' }) }));

const session = client.session.getState();
const state = () => useWorkbench.getState();
const listeners: { feed: string; listener: FeedListener; unsubscribe: ReturnType<typeof vi.fn> }[] = [];
let doc: Document, heading: DocumentBlock, snapshot: DocumentSync, viewId: string;
const refused = (code: 'not_found' | 'authz.denied' | 'internal') => new ApiError({ type: `orch:error:${code}`, code,
  status: code === 'not_found' ? 404 : code === 'authz.denied' ? 403 : 500,
  title: 'Private native diagnostic', detail: 'Private native diagnostic', request_id: 'private-request' }, null);

function NativeOwner({ id }: { id: string | null }) { useDocumentFeed(id); return null; }
function Frame({ duplicateOwner = false }: { duplicateOwner?: boolean }) {
  const layout = useWorkbench((value) => value.layout), active = useWorkbench((value) => value.activeContent);
  const tab = findTab(layout, viewId)?.tab, Body = viewOf('comments')?.body;
  const root = useRef<HTMLDivElement>(null);
  if (tab === undefined || Body === undefined) throw new Error('The real registered Comments view is required.');
  const target = panelTarget(tab, layout, active, null);
  const id = target?.kind === 'document' ? target.id : null;
  // Content and inspector views share the same native follower for their addressed document.
  return <><NativeOwner id={id} />{duplicateOwner && <NativeOwner id={id} />}
    <ViewInstanceContext.Provider value={{ id: tab.id, root, target, headingOwner: tab.scope?.mode === 'fixed' ? tab.id : active, content: false, focusedContent: false }}>
      <DocumentMarksProvider owner={tab.id} documentId={id ?? ''}>
        <section data-testid="comments"><Body tab={tab} area="right" /></section>
        <section data-testid="outline">{id !== null && <DocumentOutline documentId={id} />}</section>
      </DocumentMarksProvider>
    </ViewInstanceContext.Provider></>;
}
const panel = (name: 'comments' | 'outline') => within(screen.getByTestId(name));
const noWrites = () => { expect(calls.start).not.toHaveBeenCalled(); expect(calls.reply).not.toHaveBeenCalled(); expect(calls.edit).not.toHaveBeenCalled(); };

beforeEach(() => {
  state().leave(); feeds.clear(); documentWriter.clear(); retireCommentDraftSession(); retireDocumentMarks(); localStorage.clear();
  calls.read.mockReset(); calls.start.mockReset(); calls.reply.mockReset(); calls.edit.mockReset(); listeners.length = 0;
  const world = new World(), person = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((value) => value.owner === 'ada' && !value.deleted);
  if (person === undefined || stored === undefined) throw new Error('A readable native document fixture is required.');
  const native = world.documents.sync(person, stored, null), block = native.blocks[0];
  if (block === undefined) throw new Error('The native document needs one block.');
  doc = { ...world.documents.full(person, stored), title: 'Confidential document title' };
  heading = { ...block, type: 'heading', parent_id: null, props: { level: 1 }, content: [{ type: 'text', text: 'Confidential heading' }] };
  const discussion: DocumentDiscussion = { id: 'inspector-discussion', document_id: doc.id, block_id: heading.id,
    quote: '', anchored: true, resolved: null, created_by: doc.owner, created_at: doc.created_at, seq_updated: doc.seq,
    comments: [{ id: 'inspector-comment', discussion_id: 'inspector-discussion', author: doc.owner,
      content: [{ type: 'text', text: 'Confidential comment' }], reactions: [], edited_at: null, deleted: false, created_at: doc.created_at, seq_updated: doc.seq }] };
  snapshot = { ...native, document: doc, blocks: [heading], discussions: [discussion] };
  client.session.setState({ status: 'signed_in', userId: doc.owner.id, readerVersion: (session.readerVersion ?? 0) + 1 });
  state().enter(doc.owner.id, 'document'); state().change((layout) => ({ ...layout, main: makeGroup() }));
  state().openTab('page', { href: `/doc/${doc.id}` }); viewId = state().openTab('comments', undefined, 'new');
  vi.spyOn(socket, 'subscribe').mockImplementation((feed, listener) => {
    const held = { feed, listener, unsubscribe: vi.fn() }; listeners.push(held); return held;
  });
});
afterEach(() => {
  cleanup(); feeds.clear(); state().leave(); documentWriter.clear();
  forgetCommentDraftDeviceData(doc.owner.id, null); retireCommentDraftSession(); retireDocumentMarks();
  vi.restoreAllMocks(); vi.useRealTimers(); localStorage.clear(); client.session.setState(session);
});

it.each(['not_found', 'authz.denied'] as const)('presents initial %s in both inspectors and stays quiet while duplicate content owners share one failed read', async (code) => {
  calls.read.mockRejectedValue(refused(code)); const drawn = show(<Frame duplicateOwner />);
  await panel('comments').findByRole('alert'); await panel('outline').findByRole('alert');
  expect(panel('comments').queryByText(i18n.t('common.loading'))).toBeNull();
  expect(screen.queryByText('Private native diagnostic')).toBeNull();
  expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).toBeNull();
  expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.read.mock.calls[0]?.[0]).toBe(doc.id); expect(listeners).toHaveLength(0);
  vi.useFakeTimers();
  await act(async () => { await vi.advanceTimersByTimeAsync(120_000); });
  drawn.rerender(<Frame duplicateOwner />);
  expect(calls.read).toHaveBeenCalledTimes(1); expect(documentStateNow(doc.id).error).toBeInstanceOf(ApiError); noWrites();
});

it.each(['comments', 'outline'] as const)('%s retries a transient native failure through the same shared owner and recovers both views once', async (surface) => {
  calls.read.mockRejectedValueOnce(refused('internal')).mockResolvedValueOnce(snapshot);
  show(<Frame duplicateOwner />);
  const retry = await panel(surface).findByRole('button', { name: i18n.t('common.tryAgain') });
  const first = calls.read.mock.calls[0]?.[2];
  expect(calls.read).toHaveBeenCalledTimes(1);
  fireEvent.click(retry);
  await screen.findByText('Confidential comment'); await panel('outline').findByText('Confidential heading');
  expect(calls.read.mock.calls.map(([id, since]) => [id, since])).toEqual([[doc.id, null], [doc.id, null]]);
  expect(first?.aborted).toBe(true); expect(listeners.map((entry) => entry.feed)).toEqual([`doc:${doc.id}`]);
  expect(panel('comments').queryByRole('alert')).toBeNull(); expect(panel('outline').queryByRole('alert')).toBeNull(); noWrites();
});

it.each(['rows', 'error'] as const)('manual retry retires a pending load so its late %s cannot replace the recovered inspector', async (late) => {
  let resolveOld!: (value: DocumentSync) => void, rejectOld!: (error: unknown) => void;
  const old = new Promise<DocumentSync>((resolve, reject) => { resolveOld = resolve; rejectOld = reject; });
  calls.read.mockReturnValueOnce(old).mockResolvedValueOnce(snapshot);
  documentWriter.failed(doc.id, refused('internal')); show(<Frame />);
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
  const first = calls.read.mock.calls[0]?.[2];
  fireEvent.click(panel('comments').getByRole('button', { name: i18n.t('common.tryAgain') }));
  await screen.findByText('Confidential comment');
  await act(async () => {
    if (late === 'rows') resolveOld({ ...snapshot, document: { ...doc, title: 'Obsolete title' }, blocks: [], discussions: [] });
    else rejectOld(refused('not_found'));
    await Promise.resolve();
  });
  expect(first?.aborted).toBe(true); expect(documentStateNow(doc.id).document).toBe(doc); expect(documentStateNow(doc.id).error).toBeNull();
  expect(screen.getByText('Confidential comment')).toBeVisible(); expect(calls.read).toHaveBeenCalledTimes(2); expect(listeners).toHaveLength(1); noWrites();
});

it('suppresses cached denied content and names while retaining the exact local draft and its save guard through recovery', async () => {
  calls.read.mockResolvedValue(snapshot); show(<Frame />); await screen.findByText('Confidential comment');
  const drafts = getCommentDrafts(doc.owner.id, viewId, doc.id), target = { kind: 'new' as const, block: heading.id, quote: '' };
  const write = localStorage.setItem.bind(localStorage);
  const blocked = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key: string, value: string) {
    if (key === drafts.key) throw new DOMException('Device write refused', 'SecurityError'); write(key, value);
  });
  act(() => { setCommentText(drafts, target, 'Retain this local unsent comment'); });
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('Retain this local unsent comment');
  act(() => { documentWriter.failed(doc.id, refused('authz.denied')); });
  expect(panel('comments').queryByRole('textbox')).toBeNull();
  expect(screen.queryByText('Confidential comment')).toBeNull(); expect(screen.queryByText('Confidential heading')).toBeNull();
  expect(screen.queryByText('Confidential document title', { exact: false })).toBeNull();
  expect(panel('comments').getByRole('button', { name: i18n.t('documents.comments.recoverOther', { title: i18n.t('documents.comments.recoverDocument') }) })).toBeVisible();
  expect(drafts.store.getState().data.slots[commentSlotKey(target)]?.text).toBe('Retain this local unsent comment');
  expect(state().unsaved.has(viewId)).toBe(true);
  blocked.mockRestore();
  await act(async () => { expect(await commentDraftGuard(viewId).save()).toBe(true); });
  expect(state().unsaved.has(viewId)).toBe(false); expect(localStorage.getItem(drafts.key)).toContain('Retain this local unsent comment');
  // A later admitted native snapshot restores presentation; no automatic retry or comment publication is needed.
  act(() => { documentWriter.reset(doc.id, snapshot, snapshot.seq); });
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('Retain this local unsent comment');
  expect(calls.read).toHaveBeenCalledTimes(1); noWrites();
});

it.each(['deleted', 'revoked'] as const)('shows the native %s end state in both inspectors without stale content or another read', async (reason) => {
  calls.read.mockResolvedValue(snapshot); show(<Frame />); await screen.findByText('Confidential comment');
  await waitFor(() => expect(listeners).toHaveLength(1));
  act(() => { listeners[0]?.listener.onFrame({ op: 'closed', feed: `doc:${doc.id}`, id: 1, reason }); });
  const message = i18n.t(reason === 'deleted' ? 'documents.page.endedDeleted' : 'documents.page.endedRevoked');
  expect(panel('comments').getByText(message)).toBeVisible(); expect(panel('outline').getByText(message)).toBeVisible();
  expect(screen.queryByText('Confidential comment')).toBeNull(); expect(screen.queryByText('Confidential heading')).toBeNull();
  expect(calls.read).toHaveBeenCalledTimes(1); noWrites();
});

it('keeps fixed and follow scopes distinct and shows an empty Comments state for an incompatible target without reading the browser route', async () => {
  const otherId = 'other-document';
  const other: DocumentSync = { ...snapshot, document: { ...doc, id: otherId }, blocks: [], discussions: [] };
  calls.read.mockImplementation((id) => id === doc.id ? Promise.resolve(snapshot) : id === otherId ? Promise.resolve(other) : Promise.reject(new Error(`Unexpected read ${id}`)));
  setPanelScope(viewId, { mode: 'fixed', target: { kind: 'document', id: doc.id } });
  show(<Frame />); await screen.findByText('Confidential comment');
  act(() => { state().openTab('page', { href: `/doc/${otherId}` }); });
  expect(screen.getByText('Confidential comment')).toBeVisible(); expect(calls.read).toHaveBeenCalledTimes(1);
  act(() => { setPanelScope(viewId, { mode: 'follow' }); });
  await waitFor(() => expect(documentStateNow(otherId).loaded).toBe(true));
  expect(calls.read.mock.calls.map(([id]) => id)).toEqual([doc.id, otherId]);
  expect(screen.queryByText('Confidential comment')).toBeNull();
  act(() => { state().openTab('page', { href: '/thread/another-resource' }); });
  expect(panel('comments').getByText(i18n.t('views.comments.about'))).toBeVisible();
  expect(panel('comments').queryByText(i18n.t('common.loading'))).toBeNull();
  expect(calls.read.mock.calls.map(([id]) => id)).toEqual([doc.id, otherId]); noWrites();
});
