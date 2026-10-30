/** A folder's identity stays with the frame when its browser body is hidden. */
import { Folder } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useTreeEntry } from '@/features/documents/useTreeEntry';
import { useRouteHeading } from '@/shell/screen';
import { nodeQuery } from '@/api/nodes';

export function FolderContext({ id, retained = false }: { id: string; retained?: boolean }) {
  const { t } = useTranslation();
  const folder = useTreeEntry({ kind: 'container', id }, retained);
  const directory = folder.data?.directory;
  useRouteHeading({ title: directory?.folder?.entry.title ?? t('documents.list.title'), icon: Folder,
    nodeId: directory?.node.redacted === false ? directory.node.id : undefined,
    path: directory?.path, leaf: true,
    views: { ready: directory !== undefined && directory !== null, chat: false, workspace: false, terminal: false, processes: false } });
  return null;
}

export function DocumentsContext({ id }: { id: string }) {
  const { t } = useTranslation();
  const node = useQuery(nodeQuery(id));
  useRouteHeading({ title: t('documents.list.title'), icon: Folder, nodeId: node.data?.id, path: node.data?.path,
    leaf: true, views: { ready: node.data !== undefined, chat: false, workspace: false, terminal: false, processes: false } });
  return null;
}
