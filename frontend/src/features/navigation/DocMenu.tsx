/** A document's menu where it stands in the tree: open it, rename it, move it, share it, take it away as a file, copy its link, delete it.
 *
 * The list is made only for the row that is asked, as a place's is, and from what the caller holds on
 * that document: what they may not do is left out. The acts that need a dialog open it through the
 * one host of the frame, so that the dialog outlives the menu it was asked from.
 */
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { Copy, Ellipsis, ExternalLink, FolderInput, Pencil, Share2, Trash2 } from 'lucide-react';
import type { ReactElement } from 'react';
import { useTranslation } from 'react-i18next';

import { type Document, documentQuery } from '@/api/documents';
import { nodeQuery } from '@/api/nodes';
import { openAccess } from '@/features/access/AccessDialog';
import { useReadmeEntry } from '@/features/documents/Readme';
import { openExport, requestExportAccess } from '@/features/export/ExportHost';
import { exportEntries } from '@/features/export/menu';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { type DocPlace, documentPath } from '@/features/navigation/places';
import { holds } from '@/lib/permissions';
import { copyText } from '@/ui/clipboard';
import { ContextEntries, ContextMenu } from '@/ui/ContextMenu';
import { IconButton } from '@/ui/IconButton';
import { DropdownEntries, Menu, type MenuEntry } from '@/ui/Menu';

/** The acts, each as the thing to call; the list says which are offered. */
export interface DocMenuActions {
  open: () => void;
  openInTab: () => void;
  rename: () => void;
  move: () => void;
  share: () => void;
  export: () => void;
  requestExport: () => void;
  copyLink: () => void;
  remove: () => void;
  readme?: MenuEntry;
}

/**
 * The entries of a document's menu in a row: where it leads, how it is changed, who else gets it, and
 * last, apart, what destroys. `held` is what the caller holds on it, `undefined` while that has not
 * arrived: then only what needs no permission is listed, with a line that says the rest is being asked.
 */
export function docMenuEntries(doc: Pick<DocPlace, 'archived' | 'owner'>, held: Pick<Document, 'access'> | undefined, act: DocMenuActions, t: TFunction): MenuEntry[] {
  const entries: MenuEntry[] = [];
  const group = (...next: MenuEntry[]) => {
    if (next.length === 0) return;
    if (entries.length > 0) entries.push({ type: 'separator', id: `separator-${entries.length}` });
    entries.push(...next);
  };
  const access = held?.access;
  group(
    { id: 'open', label: t('places.menu.open'), onSelect: act.open },
    { id: 'open-tab', label: t('places.menu.openInTab'), icon: <ExternalLink aria-hidden />, shortcut: 'Mod+Enter', onSelect: act.openInTab },
  );
  group(
    // A document beside an archived place is read and not written: its title is part of what is written.
    ...(holds(access, 'doc.edit') && !doc.archived ? [{ id: 'rename', label: t('places.menu.rename'), icon: <Pencil aria-hidden />, onSelect: act.rename }] : []),
    ...(holds(access, 'doc.manage') ? [{ id: 'move', label: t('places.menu.move'), icon: <FolderInput aria-hidden />, onSelect: act.move }] : []),
    ...(act.readme === undefined ? [] : [act.readme]),
  );
  group(
    ...(access === undefined ? [] : [{ id: 'share', label: t('access.menu.share'), icon: <Share2 aria-hidden />, onSelect: act.share }]),
    ...exportEntries({ allowed: access === undefined ? null : holds(access, 'doc.export'), owner: doc.owner, where: 'row', onExport: act.export, onRequest: act.requestExport, t }),
    { id: 'copy-link', label: t('places.menu.copyLink'), icon: <Copy aria-hidden />, onSelect: act.copyLink },
  );
  group(...(holds(access, 'doc.manage') ? [{ id: 'delete', label: t('places.menu.delete'), icon: <Trash2 aria-hidden />, danger: true, onSelect: act.remove }] : []));
  if (access === undefined) group({ type: 'label', id: 'checking', label: t('places.menu.checking') });
  return entries;
}

/** The whole list of a document's menu, with its dialogs wired. Drawn only while a menu is open: it asks what the caller holds. */
function useDocMenu(doc: DocPlace): MenuEntry[] {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const held = useQuery(documentQuery(doc.id)).data;
  const place = useQuery({ ...nodeQuery(held?.node.id ?? ''), enabled: held !== undefined }).data;
  const readme = useReadmeEntry(doc, place);
  const target = { kind: 'document' as const, id: doc.id, name: doc.title };
  const copy = () => copyText(new URL(documentPath(doc.id), location.origin).href, t('common.linkCopied'));
  return docMenuEntries(
    doc,
    held,
    {
      open: () => void navigate({ to: '/doc/$documentId', params: { documentId: doc.id } }),
      openInTab: () => window.open(documentPath(doc.id), '_blank', 'noopener'),
      rename: () => openNodeDialog({ type: 'rename-document', doc }),
      move: () => openNodeDialog({ type: 'move-document', doc }),
      share: () => openAccess(target),
      export: () => openExport(target),
      requestExport: () => requestExportAccess({ ...target, owner: doc.owner }),
      copyLink: () => void copy(),
      remove: () => openNodeDialog({ type: 'delete-document', doc }),
      ...(readme === null ? {} : { readme }),
    },
    t,
  );
}

function ButtonEntries({ doc, extraEntries = [] }: { doc: DocPlace; extraEntries?: MenuEntry[] | undefined }) {
  return <DropdownEntries entries={[...useDocMenu(doc), ...extraEntries]} />;
}

function ContextList({ doc, extraEntries = [] }: { doc: DocPlace; extraEntries?: MenuEntry[] | undefined }) {
  return <ContextEntries entries={[...useDocMenu(doc), ...extraEntries]} />;
}

/** The "⋯" of a document's row: the same list as its right-click menu. */
export function DocMenuButton({ doc, skipTab = false, extraEntries }: { doc: DocPlace; skipTab?: boolean; extraEntries?: MenuEntry[] | undefined }) {
  const { t } = useTranslation();
  return (
    <Menu
      label={t('places.menu.of', { name: doc.title })}
      align="end"
      content={<ButtonEntries doc={doc} extraEntries={extraEntries} />}
      trigger={<IconButton size="sm" label={t('places.menu.more', { name: doc.title })} icon={<Ellipsis aria-hidden />} {...(skipTab ? { tabIndex: -1 } : {})} />}
    />
  );
}

/** Gives whatever it is put round the right-click menu of a document. */
export function DocContextMenu({ doc, children, extraEntries }: { doc: DocPlace; children: ReactElement; extraEntries?: MenuEntry[] | undefined }) {
  const { t } = useTranslation();
  return (
    <ContextMenu label={t('places.menu.of', { name: doc.title })} content={<ContextList doc={doc} extraEntries={extraEntries} />}>
      {children}
    </ContextMenu>
  );
}
