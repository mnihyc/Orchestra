/** What every view of a thread beside its conversation shares: the door into the thread's environment with its three states, and the way a list of paths is drawn.
 *
 * A view of the workspace or of the runtime is of one thread. While the thread loads it shows a
 * skeleton; on a screen that is no thread's, or for a thread that works nowhere yet, it says so; and for a reader
 * who lacks what the view needs it says what it takes and whom to ask, as the API says it.
 */
import { Folder } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { type WorkspaceAccess, usePanelWorkspaceDoor, workspaceDenial } from '@/features/files/door';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export type Need = 'files.view' | 'terminal.view' | 'terminal.manage' | 'thread.view';

const HELD: Record<Need, (access: WorkspaceAccess) => boolean> = {
  'files.view': (access) => access.mayView,
  'terminal.view': (access) => access.mayWatchProcesses,
  'terminal.manage': (access) => access.mayShell,
  'thread.view': () => true,
};

export interface ThreadViewFrameProps {
  /** What the view needs of the thread's page. */
  need: Need;
  icon: ReactNode;
  children: (access: WorkspaceAccess) => ReactNode;
}

/** The body of a view that stands on a thread's door: what it draws once the door is open and the reader may use it. */
export function ThreadViewFrame({ need, icon, children }: ThreadViewFrameProps) {
  const { t } = useTranslation();
  // These are current workspace/runtime facts, not a second conversation-history selection.
  const state = usePanelWorkspaceDoor();
  if (state.kind === 'error') return <ErrorState compact error={state.error} />;
  if (state.kind === 'loading') return <SkeletonRows rows={6} />;
  if (state.kind === 'none') {
    return (
      <EmptyState
        compact
        className="h-full"
        icon={icon}
        title={state.why === 'no_thread' ? t('inspector.frame.noThread') : t('inspector.frame.noEnvironment')}
        description={state.why === 'no_thread' ? t('inspector.frame.noThreadAbout') : t('inspector.frame.noEnvironmentAbout')}
      />
    );
  }
  if (state.access.thread === null) return <EmptyState compact className="h-full" icon={icon} title={t('inspector.frame.noThread')} />;
  const access: WorkspaceAccess = { ...state.access, thread: state.access.thread };
  if (!HELD[need](access)) {
    const missing = workspaceDenial(t, need, access.environment);
    return <EmptyState compact className="h-full" icon={icon} title={missing} />;
  }
  return <>{children(access)}</>;
}

/** A path as a name with the directory it lies in beneath or beside it, which is how a list of changes is read. */
export function PathLabel({ path }: { path: string }) {
  const slash = path.lastIndexOf('/');
  const name = path.slice(slash + 1);
  const directory = slash < 0 ? '' : path.slice(0, slash);
  return (
    <span className="flex min-w-0 flex-1 items-baseline gap-2" title={path}>
      <span className="truncate font-medium text-text">{name}</span>
      {directory !== '' && <span className="min-w-0 truncate text-xs text-text-muted">{directory}</span>}
    </span>
  );
}

export const FOLDER_ICON = <Folder aria-hidden />;
