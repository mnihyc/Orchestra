/** A path that an item of the conversation names, opening its file in the code view where the thread may look at files.
 *
 * What an agent writes in a tool call is a path as it knew it: absolute, or relative to the directory it
 * works in. The link says where that is in the workspace and opens it as a tab, at the line the item
 * names; a path that lies outside the workspace, or a reader who may not look at files, gets the plain text.
 */
import { useTranslation } from 'react-i18next';
import { useQuery } from '@tanstack/react-query';

import { environmentQuery } from '@/api/workspace';
import { useTimelineContext } from '@/features/timeline/context';
import { useThreadRow } from '@/realtime/stores/thread';
import { cx } from '@/ui/cx';

import { openFile } from './FileTabs';
import { FileAttachment } from './FileChip';
import { FileChip } from './FileChip';
import { workspaceSource } from './source';
import { useFileVisible } from './FilePreview';

function currentRoot(registered: string, facts: unknown, generation: number): string {
  if (typeof facts !== 'object' || facts === null || !('generation' in facts) || facts.generation !== generation || !('workspace_root' in facts) || typeof facts.workspace_root !== 'string') return registered;
  const root = facts.workspace_root.replace(/\\/g, '/');
  return root.startsWith('/') || /^[a-z]:\//i.test(root) ? root : registered;
}

function atRoot(path: string, registered: string, current: string): string {
  const base = registered.replace(/\\/g, '/').replace(/\/+$/, '');
  const value = path.replace(/\\/g, '/');
  const windows = /^[a-z]:/i.test(base);
  const matches = windows ? value.toLowerCase() : value;
  const prefix = windows ? base.toLowerCase() : base;
  if (matches === prefix) return current;
  return matches.startsWith(`${prefix}/`) ? `${current.replace(/\/+$/, '')}${value.slice(base.length)}` : path;
}

/** Where a path of a tool call is in the workspace: from the workspace root, without `.` and `..`; `null` where it lies outside it. */
export function workspacePath(path: string, cwd: string, directory: string, workspaceRoot?: string): string | null {
  path = path.replace(/\\/g, '/');
  cwd = cwd.replace(/\\/g, '/');
  directory = directory.replace(/\\/g, '/');
  const suffix = directory === '' ? '' : `/${directory}`;
  const givenRoot = workspaceRoot?.replace(/\\/g, '/') ?? (suffix !== '' && cwd.endsWith(suffix) ? cwd.slice(0, cwd.length - suffix.length) : cwd);
  const root = givenRoot.replace(/\/+$/, '') || (givenRoot.startsWith('/') || workspaceRoot === undefined && cwd.startsWith('/') ? '/' : '');
  if (root === '') return null;
  const windows = /^[a-z]:/i.test(root);
  const begins = (value: string, prefix: string) => windows ? value.toLowerCase().startsWith(prefix.toLowerCase()) : value.startsWith(prefix);
  const prefix = root === '/' ? '/' : `${root}/`;
  const from = workspaceRoot === undefined ? directory : cwd.length === root.length && begins(cwd, root) ? '' : begins(cwd, prefix) ? cwd.slice(prefix.length) : null;
  let relative: string;
  if (path.startsWith('/') || /^[a-z]:\//i.test(path)) {
    if (!begins(path, prefix)) return null;
    relative = path.slice(prefix.length);
  } else {
    if (from === null) return null;
    relative = from === '' ? path : `${from}/${path}`;
  }
  const parts: string[] = [];
  for (const part of relative.split('/')) {
    if (part === '' || part === '.') continue;
    if (part === '..') {
      if (parts.length === 0) return null;
      parts.pop();
    } else parts.push(part);
  }
  return parts.length === 0 ? null : parts.join('/');
}

export interface FileLinkProps {
  /** The originating thread for prose outside the timeline's own context. */
  threadId?: string;
  path: string;
  /** The line the item names. */
  line?: number | null;
  /** What is drawn in place of the path, where the item says more than the path alone. */
  children?: React.ReactNode;
  className?: string;
}

export function FileLink({ path, line = null, children, className, threadId: origin }: FileLinkProps) {
  const { t } = useTranslation();
  const { threadId: contextThread, inspection } = useTimelineContext();
  const threadId = origin ?? contextThread;
  const thread = useThreadRow(threadId);
  const label = children ?? path;
  const environment = thread?.environment ?? null;
  const mayRead = inspection === undefined && thread?.environment_permissions.includes('files.view') === true;
  const root = useQuery({ ...environmentQuery(environment?.id ?? ''), enabled: environment !== null && mayRead });
  const workspace = root.data === undefined ? null : currentRoot(root.data.workspace_root, root.data.facts, root.data.generation);
  const cwd = thread === null || root.data === undefined || workspace === null ? null : atRoot(thread.cwd, root.data.workspace_root, workspace);
  const where = thread === null || workspace === null || cwd === null ? null : workspacePath(path, cwd, thread.directory, workspace);
  if (inspection !== undefined || thread === null || environment === null || where === null || !thread.environment_permissions.includes('files.view')) return <>{label}</>;
  const environmentId = environment.id;
  return <span className={cx('max-w-full font-mono', className)} title={t('files.link.open', { path: where })}>
    <FileChip source={{ ...workspaceSource({ environmentId, threadId }, where), line: line ?? undefined }} label={label} ariaLabel={typeof label === 'string' ? label : line === null ? path : `${path}:${line}`} presentation={{ place: origin === undefined ? 'compact' : 'prose' }} bare onOpen={() => openFile({ environmentId, threadId, path: where, line, permanent: false })} />
  </span>;
}

/** A file a tool read or wrote uses the same preview and viewer as a human attachment. */
export function FilePathAttachment({ path, name = path.split('/').at(-1) ?? path, size, mediaType }: { path: string; name?: string; size?: number; mediaType?: string }) {
  const { ref, visible } = useFileVisible();
  const { threadId, inspection } = useTimelineContext();
  const thread = useThreadRow(threadId);
  const environment = thread?.environment ?? null;
  const mayRead = inspection === undefined && thread?.environment_permissions.includes('files.view') === true;
  const root = useQuery({ ...environmentQuery(environment?.id ?? ''), enabled: environment !== null && mayRead && visible });
  const workspace = root.data === undefined ? null : currentRoot(root.data.workspace_root, root.data.facts, root.data.generation);
  const cwd = thread === null || root.data === undefined || workspace === null ? null : atRoot(thread.cwd, root.data.workspace_root, workspace);
  const where = thread === null || workspace === null || cwd === null ? null : workspacePath(path, cwd, thread.directory, workspace);
  return <span ref={ref}>{!mayRead || environment === null || where === null ? name : <FileAttachment source={{ ...workspaceSource({ environmentId: environment.id, threadId }, where, name, size), mediaType }} />}</span>;
}
