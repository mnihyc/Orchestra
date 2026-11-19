/** Browse the document tree without treating its folders as hierarchy nodes or text editors. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { Copy, FilePlus2, FileText, FileUp, Folder, FolderPlus, MoreHorizontal, Pencil, Share2, Trash2, Undo2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { createDocumentInFolder, documentTreeEntryQuery, organizeDocuments } from '@/api/documentTree';
import { createDocument } from '@/api/documents';
import { useActKey } from '@/lib/forms';
import { TreeActionDialog, type TreeAction } from '@/features/documents/TreeActions';
import { openAccess } from '@/features/access/AccessDialog';
import { documentAncestors, documentsPath, folderPath } from '@/features/documents/treeLocation';
import { DocumentRow } from '@/features/documents/DocumentRow';
import { useDirectoryPage } from '@/features/documents/useDirectoryPage';
import type { DocumentDirectory, DocumentTreeListed } from '@/generated/api-types';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Breadcrumbs, type Crumb } from '@/ui/Breadcrumbs';
import { Button } from '@/ui/Button';
import { ContextMenu } from '@/ui/ContextMenu';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { SkeletonRows } from '@/ui/Skeleton';
import { copyText } from '@/ui/clipboard';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { RelativeTime } from '@/ui/RelativeTime';
import { toast } from '@/ui/Toast';
import { Tooltip } from '@/ui/Tooltip';
import { InspectionBanner } from '@/features/trace/InspectionBanner';
import { openImport } from '@/features/people/importHostState';

/** The typed folder reader supplies its anchor; no hierarchy permission is inferred. */
export function DocumentFolderPage({ id, retained = false }: { id: string; retained?: boolean }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const folder = useQuery(documentTreeEntryQuery({ kind: 'container', id }, cache, retained));
  if (folder.isError) return <Page><ErrorState error={folder.error} onRetry={() => void folder.refetch()}
    action={retained ? undefined : <Button asChild><Link to="/inspect/$kind/$id" params={{ kind: 'document_container', id }}>{t('trace.inspect')}</Link></Button>} /></Page>;
  const directory = folder.data?.directory;
  if (folder.isPending) return <Page><SkeletonRows rows={5} /></Page>;
  if (directory === undefined || directory === null) return <Page><ErrorState error={new Error('The folder metadata omitted its directory')} message={t('documentFolders.unavailable')} onRetry={() => void folder.refetch()} /></Page>;
  return <DocumentDirectoryPage node={directory.node.id} at={{ kind: 'container', id }} retained={retained} />;
}

export function DocumentDirectoryPage({ node, at, retained = false }: { node: string; at: DocumentDirectory; retained?: boolean }) {
  const { t } = useTranslation();
  const me = useStore(client.session, (state) => state.userId);
  const navigate = useNavigate();
  const key = useActKey();
  const [deleted, setDeleted] = useState(retained);
  const [action, setAction] = useState<{ command: TreeAction; rows: DocumentTreeListed[] } | null>(null);
  const view = useDirectoryPage({ node, at, deleted });
  const make = useMutation({
    mutationFn: () => {
      const body = { title: t('documents.page.untitled'), access: 'follows' as const };
      const token = key({ at, body });
      return at.kind === 'container' ? createDocumentInFolder(at.id, body, token) : createDocument(at.id, body, token);
    },
    onSuccess: (made) => { void navigate({ to: '/doc/$documentId', params: { documentId: made.id }, search: { new: true } }); },
  });
  const restore = useMutation({
    mutationFn: (row: DocumentTreeListed) => {
      const body = { command: 'restore' as const, items: [row.entry.item], to: null, name: null, version: null, deletion_plan: null };
      return organizeDocuments({ ...body, client_id: key(body) });
    },
    onSuccess: (_receipt, row) => {
      toast({ tone: 'success', title: t('documents.list.restored') });
      if (retained && row.entry.item.kind === 'container' && row.entry.item.id === at.id) void navigate({ to: '/document-folder/$folderId', params: { folderId: at.id } });
    },
  });
  const page = view.query.data;
  const directory = page?.directory;
  const title = directory?.folder?.entry.title ?? t('documents.list.title');
  useScreenHeading({ title, icon: Folder, leaf: true, path: directory?.path,
    nodeId: directory?.node.redacted === false ? directory.node.id : undefined,
    views: { ready: directory !== undefined, chat: false, workspace: false, terminal: false, processes: false } });
  const crumbs: Crumb[] = directory === undefined ? [] : [
    ...(at.kind === 'container' ? [{ id: `docs:${directory.node.id}`, label: t('documents.list.title'),
      href: retained || directory.node.redacted ? undefined : documentsPath(directory.node.id) }] : []),
    ...documentAncestors(directory.folder?.entry.parent ?? directory.at, directory.ancestors).map((ancestor) => ({
      id: ancestor.id, label: ancestor.title ?? t('ui.nameUnknown'),
      href: ancestor.standing.read ? folderPath(ancestor.id, retained) : undefined,
    })),
  ];
  const menuFor = (row: DocumentTreeListed): MenuEntry[] => {
    const { item } = row.entry;
    const kind = item.kind === 'container' ? 'document_container' as const : 'document' as const;
    const path = item.kind === 'container' ? folderPath(item.id) : `/doc/${encodeURIComponent(item.id)}`;
    if (row.entry.deleted_at !== null) return [{ id: 'inspect', label: t('trace.inspect'), onSelect: () => { void navigate({ to: '/inspect/$kind/$id', params: { kind, id: item.id } }); } }, { id: 'restore', label: t('documents.list.restore'), icon: <Undo2 aria-hidden />, disabled: restore.isPending ? t('documentFolders.pending') : row.standing.manage ? false : t('documents.menu.needsManage'), onSelect: () => restore.mutate(row) }];
    return [
      { id: 'link', label: t('files.menu.copyLink'), onSelect: () => { void copyText(new URL(path, location.origin).href); } },
      { id: 'access', label: t('documentFolders.access'), icon: <Share2 aria-hidden />,
        onSelect: () => openAccess({ kind, id: item.id, name: row.entry.title }, { share: row.standing.manage }) },
      { id: 'copy', label: t('documentFolders.actions.copy'), icon: <Copy aria-hidden />, onSelect: () => setAction({ command: 'copy', rows: [row] }) },
      { id: 'move', label: t('documentFolders.actions.move'), disabled: row.standing.manage ? false : t('documents.menu.needsManage'), onSelect: () => setAction({ command: 'move', rows: [row] }) },
      { id: 'rename', label: t('documentFolders.actions.rename'), icon: <Pencil aria-hidden />, disabled: item.kind === 'container' ? row.standing.manage ? false : t('documents.menu.needsManage') : row.standing.write ? false : t('documentFolders.needsEdit'), onSelect: () => setAction({ command: 'rename', rows: [row] }) },
      { id: 'delete', label: t('documentFolders.actions.delete'), icon: <Trash2 aria-hidden />, danger: true, disabled: row.standing.manage ? false : t('documents.menu.needsManage'), onSelect: () => setAction({ command: 'delete', rows: [row] }) },
    ];
  };
  return <Page>
    {crumbs.length > 0 && <Breadcrumbs crumbs={crumbs} />}
    <PageHeader title={title} mark={<PlaceMark><Folder aria-hidden /></PlaceMark>} actions={directory === undefined ? undefined : <>
      {!deleted && <>
        <Button icon={<FolderPlus aria-hidden />} disabled={!directory.can_create} onClick={() => setAction({ command: 'create_folder', rows: [] })}>{t('documentFolders.actions.create_folder')}</Button>
        <Tooltip content={directory.can_create ? undefined : t('importWizard.destination.folderDenied')}><Button icon={<FileUp aria-hidden />} blocked={!directory.can_create} onClick={() => openImport({ id: directory.node.id, label: directory.node.redacted ? t('ui.nameUnknown') : directory.node.name ?? t('ui.nameUnknown') }, at.kind === 'container' ? { id: at.id, label: title } : null)}>{t('people.import')}</Button></Tooltip>
        <Button variant="primary" icon={<FilePlus2 aria-hidden />} disabled={!directory.can_create} pending={make.isPending} onClick={() => make.mutate()}>{t('documents.list.new')}</Button>
      </>}
      {retained && directory.folder?.entry.deleted_at === null && at.kind === 'container' && <Button asChild><Link to="/document-folder/$folderId" params={{ folderId: at.id }}>{t('documentFolders.openCurrent')}</Link></Button>}
      {directory.folder !== null && (!retained || directory.folder.entry.deleted_at !== null) && <Menu label={t('files.menu.for', { name: title })} entries={menuFor(directory.folder)}
        trigger={<IconButton label={t('files.menu.for', { name: title })} icon={<MoreHorizontal aria-hidden />} />} />}
    </>} />
    {retained && <InspectionBanner facts={directory?.inspection} />}
    {!retained && <SegmentedControl label={t('documents.list.title')} value={deleted ? 'deleted' : 'current'}
      segments={[{ value: 'current', label: t('documentFolders.current') }, { value: 'deleted', label: t('documentFolders.deleted') }]}
      onValueChange={(value) => setDeleted(value === 'deleted')} />}
    {make.isError && <ErrorAlert error={make.error} />}
    {restore.isError && <ErrorAlert error={restore.error} />}
    {view.staleCursor ? <ErrorState error={view.query.error} message={t('documentFolders.changed')}
      action={<Button onClick={() => void view.restart().catch(() => undefined)}>{t('files.tree.reload')}</Button>} />
      : view.query.isError ? <ErrorState error={view.query.error} onRetry={() => void view.retry().catch(() => undefined)} />
      : page === undefined ? <SkeletonRows rows={5} /> : <>
        {page.items.length === 0 ? <EmptyState compact icon={<Folder aria-hidden />} title={t(page.next_cursor === null && view.cursor === null ? 'documentFolders.empty' : 'documentFolders.emptyPage')} />
          : <ul aria-label={title} className="divide-y divide-border rounded-lg border border-border bg-surface">{page.items.map((row) => {
            const item = row.entry.item, menu = menuFor(row);
            const actions = <Menu label={t('files.menu.for', { name: row.entry.title })} entries={menu}
              trigger={<IconButton label={t('files.menu.for', { name: row.entry.title })} icon={<MoreHorizontal aria-hidden />} size="sm" />} />;
            if (item.kind === 'document' && row.document !== undefined && row.document !== null) return <DocumentRow key={`document:${item.id}`} doc={row.document}
              here={directory?.node.id ?? node} where={false} me={me} name={row.entry.basename} actions={actions} menu={menu} />;
            const content = <li className="flex items-center gap-3 px-3 py-3" key={`${item.kind}:${item.id}`}>
              {item.kind === 'container' ? <Folder className="size-4 shrink-0 text-text-muted" aria-hidden /> : <FileText className="size-4 shrink-0 text-text-muted" aria-hidden />}
              {row.entry.deleted_at !== null ? <div className="min-w-0 flex-1"><p className="truncate font-medium">{row.entry.basename}</p>{(row.inspection !== undefined && row.inspection !== null) && <p className="text-xs text-text-muted">{t('documentFolders.retainedUntil')} <RelativeTime at={row.inspection.purge_at} /></p>}</div> : <Link to={item.kind === 'container' ? '/document-folder/$folderId' : '/doc/$documentId'}
                params={item.kind === 'container' ? { folderId: item.id } : { documentId: item.id }}
                className="min-w-0 flex-1 truncate font-medium text-text hover:underline">{row.entry.basename}</Link>}{row.entry.deleted_at !== null && <Button size="sm" disabled={!row.standing.manage} pending={restore.isPending} onClick={() => restore.mutate(row)}>{t('documents.list.restore')}</Button>}{actions}
            </li>;
            return <ContextMenu key={`${item.kind}:${item.id}`} label={row.entry.title} entries={menu}>{content}</ContextMenu>;
          })}</ul>}
        {(view.cursor !== null || page.next_cursor !== null) && <div className="flex flex-wrap justify-between gap-2">
          <Button disabled={view.cursor === null} onClick={() => view.go(null)}>{t('documentFolders.beginning')}</Button>
          <Button disabled={page.next_cursor === null} onClick={() => view.go(page.next_cursor)}>{t('documentFolders.next')}</Button>
        </div>}
      </>}
    {action !== null && directory !== undefined && <TreeActionDialog command={action.command} rows={action.rows} directory={directory} onClose={() => setAction(null)} onDone={() => {
      if (action.command === 'delete' && action.rows.some((row) => row.entry.item.kind === 'container' && row.entry.item.id === at.id)) {
        void navigate({ to: '/inspect/$kind/$id', params: { kind: 'document_container', id: at.id } });
      }
    }} />}
    {view.live.phase === 'waiting' && <p role="status" className="text-sm text-text-muted">{t('documentFolders.waiting')}</p>}
    {view.live.error !== null && !view.query.isError && <ErrorState compact error={view.live.error} onRetry={() => void view.retry().catch(() => undefined)} />}
  </Page>;
}
