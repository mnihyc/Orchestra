/** Workspace metadata follows the saved native source, without requiring a separate environment view. */
import { Box, File as FileIcon, FileDiff } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useWorkspaceDoor } from '@/features/files/door';
import { useRouteHeading } from '@/shell/screen';
import type { ContentLocation, WorkspaceSource } from './contentAddress';

export function WorkspaceContext({ source, location }: { source: WorkspaceSource; location?: ContentLocation | null }) {
  const { t } = useTranslation();
  const state = useWorkspaceDoor(source.threadId, source.environmentId);
  const access = state.kind === 'ready' ? state.access : null;
  const file = location !== undefined && location !== null && location.view !== 'page' ? location : null;
  const basename = file === null ? null : file.path.slice(file.path.lastIndexOf('/') + 1);
  const title = basename === null ? access?.environment.name ?? t('views.environment.title')
    : file?.view === 'difference' ? t('files.difference.tab', { name: basename }) : basename;
  useRouteHeading({ title, icon: file === null ? Box : file.view === 'difference' ? FileDiff : FileIcon,
    target: access === null ? undefined : { kind: 'environment', id: source.environmentId },
    nodeId: access?.thread?.id, path: access?.thread?.path,
    views: state.kind === 'loading' ? undefined : { ready: access !== null, workspace: access?.mayView === true,
      terminal: access?.mayShell === true, processes: access?.mayWatchProcesses === true } });
  return null;
}
