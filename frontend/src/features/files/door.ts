/** The way a thread opens into the workspace of its environment, and what its reader may do through it.
 *
 * A thread is a door into the room its environment is: whoever may look at its files reads them
 * anywhere in the workspace, and whoever may write writes anywhere in it. What the reader holds is held
 * on the environment (`files.view`, `files.manage`, `terminal.view`, `terminal.manage`; a thread's own page
 * holds none of them) and the thread says it as `environment_permissions`. Every view of the workspace
 * asks it from here, so that none of them reads a role's name.
 */
import { useViewInstance } from '@/features/workbench/instance';
import { workspaceThreadRead } from '@/features/workbench/workspaceRead';
import type { ThreadReadTarget } from '@/realtime/threadScope';
import type { TFunction } from 'i18next';
import { useParams } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';

import { type Door, environmentQuery } from '@/api/workspace';
import type { Environment, EnvironmentRef, Thread } from '@/generated/api-types';
import { denial, permissionLabel } from '@/lib/permissions';
import { useThreadFeed } from '@/realtime/live';
import { useThreadView } from '@/realtime/stores/thread';
import { useStore } from 'zustand';
import { client } from '@/api/client';
export { workspaceBinding } from '@/api/workspaceBinding';

export interface EnvironmentWorkspaceAccess {
  door: Door;
  thread: Thread | null;
  environment: EnvironmentRef;
  /** The thread's directory, from the workspace root: where the tree opens. */
  directory: string;
  mayView: boolean;
  mayManage: boolean;
  mayWatchProcesses: boolean;
  mayShell: boolean;
}

export interface WorkspaceAccess extends EnvironmentWorkspaceAccess {
  thread: Thread;
}

interface DoorLoading { kind: 'loading'; admission?: true }
export type DoorState =
  | DoorLoading
  | { kind: 'error'; error: unknown }
  /** The screen is not a thread's, or the thread works in no environment yet. */
  | { kind: 'none'; why: 'no_thread' | 'no_environment' }
  | { kind: 'ready'; access: WorkspaceAccess };

/** The thread the screen in view is, if it is one. */
export function useRouteThread(): string | null {
  const { threadId } = useParams({ strict: false });
  const instance = useViewInstance();
  if (instance !== null) return instance.target?.kind === 'thread' ? instance.target.id : null;
  return typeof threadId === 'string' ? threadId : null;
}

/** The door of a thread: the environment, the directory, and what its reader may do there. */
export function useThreadDoor(threadId: string | null, read?: ThreadReadTarget): DoorState {
  // A tab kept from another thread follows that thread too; the feed is shared, so this costs nothing where the screen follows it already.
  const target = read ?? threadId;
  useThreadFeed(target, false);
  const view = useThreadView(target ?? '');
  const reader = useStore(client.session, (state) => state.readerVersion);
  if (threadId === null) return { kind: 'none', why: 'no_thread' };
  if (view.closed !== null) return { kind: 'none', why: 'no_thread' };
  if (view.error !== null) return { kind: 'error', error: view.error };
  const thread = view.thread;
  if (thread === null || view.read !== null && (view.read.reader !== reader || !view.read.ready || view.read.controller.signal.aborted)) {
    return { kind: 'loading', admission: true };
  }
  if (thread.environment === null) return { kind: 'none', why: 'no_environment' };
  return {
    kind: 'ready',
    access: {
      door: { environmentId: thread.environment.id, threadId },
      thread,
      environment: thread.environment,
      directory: thread.directory,
      mayView: thread.environment_permissions.includes('files.view'),
      mayManage: thread.environment_permissions.includes('files.manage'),
      mayWatchProcesses: thread.environment_permissions.includes('terminal.view'),
      mayShell: thread.environment_permissions.includes('terminal.manage'),
    },
  };
}

/** A direct environment door never borrows a thread's permission or needs a readable thread. */
export function environmentWorkspaceAccess(environment: Environment): EnvironmentWorkspaceAccess {
  const permissions = environment.access.permissions;
  return {
    door: { environmentId: environment.id, threadId: null }, thread: null,
    environment: { id: environment.id, name: environment.name, owner: environment.owner, address: environment.address, status: environment.status, placement: environment.placement, ports_exposed: environment.ports_exposed },
    directory: '', mayView: permissions.includes('files.view'), mayManage: permissions.includes('files.manage'),
    mayWatchProcesses: permissions.includes('terminal.view'), mayShell: permissions.includes('terminal.manage'),
  };
}

export type WorkspaceDoorState = DoorLoading
  | { kind: 'none'; why: 'no_thread' | 'no_environment' }
  | { kind: 'ready'; access: EnvironmentWorkspaceAccess }
  | { kind: 'error'; error: unknown };

/** A fixed environment panel uses native environment standing without borrowing a thread. */
export function usePanelWorkspaceDoor(): WorkspaceDoorState {
  const instance = useViewInstance();
  const thread = useThreadDoor(useRouteThread());
  const source = instance?.target?.workspace;
  const workspace = useWorkspaceDoor(source?.threadId ?? null, source?.environmentId ?? null);
  const environmentId = instance?.target?.kind === 'environment' ? instance.target.id : null;
  const environment = useQuery({ ...environmentQuery(environmentId ?? ''), enabled: source === undefined && environmentId !== null, retry: false });
  if (source !== undefined) return workspace;
  if (environmentId === null) return thread;
  if (environment.error !== null) return { kind: 'error' as const, error: environment.error };
  if (environment.data === undefined) return { kind: 'loading' as const };
  return { kind: 'ready' as const, access: environmentWorkspaceAccess(environment.data) };
}

/** A kept file can be opened directly from an environment as well as through a thread. */
export function useWorkspaceDoor(threadId: string | null, environmentId: string | null): WorkspaceDoorState {
  const read = workspaceThreadRead(environmentId, threadId);
  const throughThread = useThreadDoor(threadId, read ?? undefined);
  const view = useThreadView(read ?? '');
  const matches = view.closed === null && throughThread.kind === 'ready' && throughThread.access.door.environmentId === environmentId;
  const waiting = threadId !== null && throughThread.kind === 'loading' && view.closed === null && view.error === null;
  // A kept file's environment is part of its identity. A later thread binding cannot retarget it.
  // The original environment may still be read through its own current native permission gate.
  const query = useQuery({ ...environmentQuery(environmentId ?? ''), enabled: !matches && !waiting && environmentId !== null, retry: false });
  if (matches) return throughThread;
  if (waiting) return { kind: 'loading', admission: true };
  if (environmentId === null) return { kind: 'none', why: 'no_environment' };
  if (query.error !== null) return { kind: 'error', error: query.error };
  if (query.data === undefined) return { kind: 'loading' };
  return { kind: 'ready', access: environmentWorkspaceAccess(query.data) };
}

/** The environment as the caller sees it: where it runs, its switches and whether it is online. */
export function useEnvironment(environmentId: string | null): { environment: Environment | undefined; failed: unknown } {
  const query = useQuery({
    ...environmentQuery(environmentId ?? ''),
    enabled: environmentId !== null,
  });
  return { environment: query.data, failed: query.error };
}

/** Whether the environment is out of reach at the moment: stopped, or its machine away. */
export function isAway(environment: Environment | undefined): boolean {
  if (environment === undefined) return false;
  return environment.status !== 'running' || environment.machine?.online === false;
}

/** The name of the directory the tree is opened at, as the panel heads itself: `web/`, or the environment's name for the root. */
export function directoryTitle(directory: string): string {
  return directory === '' ? '' : `${directory.replace(/\/+$/, '')}/`;
}

/** What an environment must give for each thing these views do, in the reader's language. */
export function workspacePermissionLabel(t: TFunction, permission: string): string {
  switch (permission) {
    case 'files.view':
      return t('files.permission.view');
    case 'files.manage':
      return t('files.permission.manage');
    case 'terminal.view':
      return t('files.permission.terminalView');
    case 'terminal.manage':
      return t('files.permission.terminalManage');
    default:
      return permissionLabel(permission);
  }
}

/** Why a reader may not do something here, as a sentence: what it takes, on which environment, and whom to ask. */
export function workspaceDenial(t: TFunction, permission: string, environment: Pick<EnvironmentRef, 'name' | 'owner'>): string {
  return denial(t, permission, environment.name, environment.owner);
}
