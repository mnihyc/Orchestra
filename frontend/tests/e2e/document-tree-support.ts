/** Synthetic contract projections for browser interaction; native authorization/atomicity have separate Rust proofs. */
import { randomUUID } from 'node:crypto';
import type { Page, WebSocketRoute } from '@playwright/test';
import type { DocumentDirectory, DocumentDirectoryContext, DocumentRef, DocumentTreeEntry, DocumentTreeListed, DocumentTreeOperationBody, DocumentTreePage, DocumentTreePlan, DocumentTreeRead, DocumentTreeReceipt, NameHit, NodeRef } from '../../src/generated/api-types';
import { id } from '../doubles/fixtures.ts';

export const TREE = { node: id(91, 1), research: id(91, 2), nested: id(91, 3), archive: id(91, 4), plan: id(7, 1), appendix: id(91, 6) };
const root: DocumentDirectory = { kind: 'node_docs', id: TREE.node };
const parent: NodeRef = { id: TREE.node, kind: 'project', name: null, redacted: true };
const path: NodeRef[] = [{ id: id(2, 1), kind: 'org', name: null, redacted: true }, parent];
const access = { read: true, write: true, manage: true };
const entry = (kind: 'container' | 'document', key: string, name: string, at: DocumentDirectory, files = 0, versions = 0): DocumentTreeListed => ({
  entry: { item: { kind, id: key }, node: TREE.node, root: id(2, 1), parent: at, basename: name, title: name, owner: id(1, 1), revision: 1,
    files, versions, deleted_at: null, deletion: null }, standing: access,
});

export async function documentTreeFixture(page: Page) {
  const rows = new Map<string, DocumentTreeListed>([
    entry('container', TREE.research, 'Research', root), entry('container', TREE.archive, 'Archive', root),
    entry('container', TREE.nested, 'Experiments', { kind: 'container', id: TREE.research }),
    entry('document', TREE.plan, 'Plan.md', { kind: 'container', id: TREE.research }, 1, 3),
    entry('document', TREE.appendix, 'Appendix.md', { kind: 'container', id: TREE.nested }, 2, 4),
  ].map((row) => [row.entry.item.id, row]));
  const plan = rows.get(TREE.plan);
  if (plan !== undefined) { plan.entry.title = 'Plan'; plan.standing = { read: true, write: true, manage: false }; }
  const requests: { method: string; path: string; query: string; body?: DocumentTreeOperationBody }[] = [];
  const sockets = new Set<{ ws: WebSocketRoute; subscriptions: Map<number, string> }>();
  let denied = false;
  const ancestors = (at: DocumentDirectory) => {
    const result = [];
    let next = at;
    while (next.kind === 'container') {
      const row = rows.get(next.id); if (row === undefined) break;
      result.unshift({ id: next.id, node: row.entry.node, root: row.entry.root, title: row.entry.title, basename: row.entry.basename, parent: row.entry.parent, standing: row.standing });
      next = row.entry.parent;
    }
    return result;
  };
  const directory = (at: DocumentDirectory): DocumentDirectoryContext => {
    const folder = at.kind === 'container' ? rows.get(at.id) ?? null : null;
    const node = folder?.entry.node ?? at.id, org = folder?.entry.root ?? id(2, 1);
    return { at, node: { ...parent, id: node }, path: [{ id: org, kind: 'org', name: null, redacted: true }, { ...parent, id: node }],
      folder, ancestors: ancestors(at), can_create: at.kind === 'node_docs' || folder?.entry.deleted_at === null,
      inspection: folder?.inspection ?? null, membership_seq: 1 };
  };
  const contents = (at: DocumentDirectory, deleted = false): DocumentTreePage => ({
    items: [...rows.values()].filter((row) => row.entry.parent.kind === at.kind && row.entry.parent.id === at.id && (row.entry.deleted_at !== null) === deleted),
    directory: directory(at), ancestors: ancestors(at), feed: `fixture-directory:${at.kind}:${at.id}:${deleted}`, next_cursor: null,
  });
  const reading = (key: string): DocumentTreeRead => {
    const row = rows.get(key);
    if (row === undefined) return { entries: [], ancestors: [] };
    return { entries: [row], ancestors: ancestors(row.entry.parent), place: { node: { ...parent, id: row.entry.node }, path: [{ id: row.entry.root, kind: 'org', name: null, redacted: true }, { ...parent, id: row.entry.node }] }, feed: `fixture-entry:${key}`,
      ...(row.entry.item.kind === 'container' ? { directory: directory({ kind: 'container', id: key }) } : {}) };
  };
  const frame = (feed: string): Record<string, unknown> => {
    const parts = feed.split(':');
    if (parts[0] === 'fixture-entry') return { entry: reading(parts[1] ?? '') };
    return { directory: contents({ kind: parts[1] === 'container' ? 'container' : 'node_docs', id: parts[2] ?? '' }, parts[3] === 'true'), restart: true };
  };
  const broadcast = () => {
    for (const { ws, subscriptions } of sockets) for (const [sub, feed] of subscriptions) ws.send(JSON.stringify(denied
      ? { op: 'closed', id: sub, feed, reason: 'revoked' }
      : { op: 'event', id: sub, feed, data: frame(feed) }));
  };
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (ws) => {
    const server = ws.connectToServer(), subscriptions = new Map<number, string>();
    const held = { ws, subscriptions }; sockets.add(held);
    ws.onMessage((raw) => {
      const value = JSON.parse(String(raw)) as { op: string; id?: number; feed?: string };
      if (value.op === 'sub' && value.feed?.startsWith('fixture-') === true && value.id !== undefined) {
        subscriptions.set(value.id, value.feed);
        ws.send(JSON.stringify({ op: 'subscribed', id: value.id, feed: value.feed, seq: null, mode: 'ephemeral' }));
        ws.send(JSON.stringify({ op: 'event', id: value.id, feed: value.feed, data: frame(value.feed) }));
      } else if (value.op === 'unsub' && value.id !== undefined && subscriptions.has(value.id)) {
        const feed = subscriptions.get(value.id); subscriptions.delete(value.id);
        ws.send(JSON.stringify({ op: 'closed', id: value.id, feed, reason: 'unsubscribed' }));
      } else server.send(raw);
    });
    ws.onClose((code, reason) => { sockets.delete(held); void server.close({ ...(code === undefined ? {} : { code }), ...(reason === undefined ? {} : { reason }) }); });
  });
  const affected = (selected: readonly string[]): DocumentTreeListed[] => {
    const all = new Set(selected); let previous = -1;
    while (previous !== all.size) { previous = all.size; for (const row of rows.values()) if (row.entry.parent.kind === 'container' && all.has(row.entry.parent.id)) all.add(row.entry.item.id); }
    return [...rows.values()].filter((row) => all.has(row.entry.item.id));
  };
  await page.route(/\/api\/v1\/(?:document-tree(?:\/|$)|nodes\/[^/]+\/document-tree|search\/names|shared(?:\?|$))/, async (route) => {
    const request = route.request(), url = new URL(request.url()), name = url.pathname.replace('/api/v1', '');
    const body = request.method() === 'POST' ? request.postDataJSON() as DocumentTreeOperationBody : undefined;
    requests.push({ method: request.method(), path: name, query: url.search, ...(body === undefined ? {} : { body }) });
    if (denied) { await route.fulfill({ status: 404, json: { type: 'orch:error:not_found', code: 'not_found', title: 'Not found', detail: '', request_id: '', status: 404 } }); return; }
    if (name === '/shared') { await route.fulfill({ json: { items: [{ kind: 'document_container', folder: { kind: 'document_container', id: TREE.research, name: 'Research', redacted: false }, path, participating: true }], partial: false, next_cursor: null } }); return; }
    if (name === '/search/names') {
      const q = (url.searchParams.get('q') ?? '').toLowerCase();
      const hits: NameHit[] = [...rows.values()].filter((row) => row.entry.item.kind === 'container' && row.entry.deleted_at === null && row.entry.title.toLowerCase().includes(q) && (url.searchParams.get('org_id') === null || row.entry.root === url.searchParams.get('org_id'))).map((row) => ({ id: row.entry.item.id, kind: 'document_container', name: row.entry.title, path: [{ id: row.entry.root, kind: 'org', name: null, redacted: true }, { ...parent, id: row.entry.node }],
        href: `/document-folder/${row.entry.item.id}`, address: null, archived: false, avatar: null, color: null, custom_color: null, owner: null, recent: false, tag: null, thread_attention: null, username: null }));
      await route.fulfill({ json: { items: hits, partial: false } }); return;
    }
    if (name.startsWith('/document-tree/entries/')) {
      const key = name.split('/').at(-1) ?? '';
      if (rows.get(key)?.entry.deleted_at !== null && url.searchParams.get('inspect_deleted') !== 'true') { await route.fulfill({ status: 404, json: { code: 'not_found', detail: 'The live entry is unavailable.' } }); return; }
      await route.fulfill({ json: reading(key) }); return;
    }
    if (name.endsWith('/document-tree')) {
      const folder = url.searchParams.get('parent_container_id');
      await route.fulfill({ json: contents(folder === null ? root : { kind: 'container', id: folder }, url.searchParams.get('deleted') === 'only') }); return;
    }
    if (body === undefined) { await route.fallback(); return; }
    const selection = affected(body.items.map((item) => item.id));
    const until = '2030-11-01T12:00:00Z';
    if (name === '/document-tree/delete/preview') {
      const plan: DocumentTreePlan = { gaining: 0, losing: 0, restore_until: until,
        stamp: { entries: selection.map((row) => row.entry), authority_epoch: 1, authority_fingerprint: 'fixture-authority', authority_until: null } };
      await route.fulfill({ json: plan }); return;
    }
    if (name !== '/document-tree/operations') { await route.fallback(); return; }
    if (body.command === 'rename' && body.items.some((item) => item.kind === 'container')) {
      await route.fulfill({ status: 400, json: { code: 'request.invalid', detail: 'Text rename takes a document, not a folder.' } }); return;
    }
    const changed: DocumentTreeEntry[] = [];
    if (body.command === 'create_folder' && body.to !== null && body.name !== null) {
      const made = entry('container', randomUUID(), body.name, body.to); rows.set(made.entry.item.id, made); changed.push(made.entry);
    } else if (body.command === 'delete') {
      if (body.deletion_plan === null) { await route.fulfill({ status: 409, json: { code: 'conflict', detail: 'A complete deletion preview is required.' } }); return; }
      for (const row of selection) { row.entry.deleted_at = '2026-10-17T00:00:00Z'; row.entry.deletion = 'fixture-deletion'; row.inspection = { deleted_at: row.entry.deleted_at, deleted_by: null, purge_at: until, readonly: true }; changed.push(row.entry); }
    } else if (body.command === 'restore') {
      for (const row of selection) { row.entry.deleted_at = null; row.entry.deletion = null; row.inspection = null; changed.push(row.entry); }
    } else if (body.command === 'copy') {
      const copies = new Map(selection.map((row) => [row.entry.item.id, randomUUID()]));
      for (const row of selection) {
        const key = copies.get(row.entry.item.id); if (key === undefined) continue;
        const copy = structuredClone(row); copy.entry.item = { ...row.entry.item, id: key };
        copy.entry.parent = body.items.some((item) => item.id === row.entry.item.id) ? body.to ?? root : { kind: 'container', id: copies.get(row.entry.parent.id) ?? row.entry.parent.id };
        if (body.items.some((item) => item.id === row.entry.item.id) && body.name !== null) copy.entry.basename = copy.entry.title = body.name;
        copy.entry.versions = 0; rows.set(key, copy); changed.push(copy.entry);
      }
    } else if (body.command === 'move' || body.command === 'rename') {
      for (const item of body.items) { const row = rows.get(item.id); if (row === undefined) continue; if (body.to !== null) row.entry.parent = body.to; if (body.name !== null) { row.entry.title = body.name; row.entry.basename = body.command === 'rename' ? `${body.name}.md` : body.name; } changed.push(row.entry); }
    }
    const current = changed.map((item) => rows.get(item.item.id)).filter((row): row is DocumentTreeListed => row !== undefined);
    const receipt: DocumentTreeReceipt = { entries: changed, view: { entries: current, ancestors: current.flatMap((row) => ancestors(row.entry.parent)) },
      operation: randomUUID(), copied: [], gaining: 0, losing: 0, restore_until: body.command === 'delete' ? until : null };
    await route.fulfill({ json: receipt });
  });
  return { requests, rows, broadcast, showDocumentFacts: (doc: DocumentRef) => {
    const row = rows.get(TREE.plan); if (row !== undefined) row.document = { ...doc, id: TREE.plan, title: row.entry.title, node: parent };
  }, revoke: () => { denied = true; broadcast(); }, root };
}
