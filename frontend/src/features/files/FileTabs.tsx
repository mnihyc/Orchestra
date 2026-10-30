/** Workspace files and differences are ordinary retained tabs in the focused main group. */
import { useQueryClient } from '@tanstack/react-query';
import { Download, File as FileIcon, FileQuestion, ScanEye } from 'lucide-react';
import { type ComponentType, Suspense, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { create } from 'zustand';
import { useStore } from 'zustand';

import { ApiError, client } from '@/api/client';
import { type Environment } from '@/generated/api-types';
import { type Tab } from '@/features/workbench/layout';
import { useTabName } from '@/features/workbench/store';
import { fileLine } from '@/features/workbench/contentAddress';
import { openContent, type ContentHistory } from '@/features/workbench/contentNavigation';
import type { ViewProps } from '@/features/workbench/views';
import { useViewShown } from '@/features/workbench/viewOpening';
import { writeViewChoice } from '@/lib/viewChoices';
import { useViewportShape } from '@/features/workbench/Narrow';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorBoundary } from '@/ui/ErrorBoundary';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Menu } from '@/ui/Menu';
import { SkeletonRows } from '@/ui/Skeleton';
import { viewerDragOver, viewerDrop } from '@/lib/drop';

import { directoryKey } from '@/realtime/stores/environment';
import { useDirectoryTree } from './useDirectoryTree';

import { downloadFile, invalidateHead, useFileHead } from './content';
import { lazyPart } from './lazy';
import { Unavailable } from './Unavailable';
import { isAway, useEnvironment, useWorkspaceDoor, type EnvironmentWorkspaceAccess, workspaceBinding, workspaceDenial } from './door';
import { chosenViewer, factsOf, rememberChoice, type ReadOnlyReason, useViewerChoice, type Viewer, type ViewerId, VIEWERS, viewersFor, type ViewerProps } from './viewers';

/** What a file tab shows: the environment, the thread it is read through, and the path. */
export interface FileTabOf {
  env: string;
  thread: string;
  path: string;
}

/** The parts of a tab of the `file` view, if it is one. */
export function fileOf(tab: Tab): FileTabOf | null {
  const of = tab.of;
  if (tab.view !== 'file' || of?.env === undefined || of.thread === undefined || of.path === undefined) return null;
  return { env: of.env, thread: of.thread, path: of.path };
}

export const baseName = (path: string): string => path.slice(path.lastIndexOf('/') + 1);
export const parentOf = (path: string): string => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');

// ---------------------------------------------------------------- where a line is asked for

interface Jump {
  line: number;
  /** A new ask for the same line goes there again. */
  nonce: number;
}

/** The line a file was asked to be opened at, by thread and path, until the tab has gone there. */
export const useJumps = create<{ jumps: Readonly<Record<string, Jump>> }>(() => ({ jumps: {} }));

let nonce = 0;

function askLine(tabId: string, line: number | null | undefined): void {
  if (line === null || line === undefined || line < 1) return;
  nonce += 1;
  useJumps.setState((state) => ({ jumps: { ...state.jumps, [tabId]: { line, nonce } } }));
}

// ---------------------------------------------------------------- opening

/** Legacy preview bookkeeping can be retired; new ordinary tabs are never preview replacements. */
export const usePreviews = create<{ ids: ReadonlySet<string> }>(() => ({ ids: new Set() }));

export interface OpenFileOptions {
  environmentId: string;
  threadId: string;
  path: string;
  line?: number | null;
  /** Compatibility with earlier file-opening callers; every tab now persists. */
  permanent?: boolean;
  /** A resolved old link replaces its alias; an ordinary explicit open pushes the new resource. */
  history?: ContentHistory;
}

/** Opens a file as a tab in the viewer for its kind. */
export function openFile(options: OpenFileOptions): void {
  const { environmentId, threadId, path } = options;
  const line = fileLine(options.line);
  const id = openContent({ view: 'file', environmentId, threadId: threadId === '' ? null : threadId, path,
    ...(line === undefined ? {} : { line }) }, 'normal', options.history ?? 'push');
  if (id === null) return;
  askLine(id, options.line);
  writeViewChoice(client.session.getState().userId, 'global', 'mobileTab', id);
  writeViewChoice(client.session.getState().userId, 'global', 'mobileView', 'file');
}

/** The tab is the person's own from now on: no later file takes its place. */
export function keepFileTab(tabId: string): void {
  usePreviews.setState((state) => (state.ids.has(tabId) ? { ids: new Set([...state.ids].filter((id) => id !== tabId)) } : state));
}

/** What a difference tab shows: a change recorded by a tool call, or the Git difference of a path. */
export interface DifferenceTabOf {
  kind: 'change' | 'git';
  env: string;
  thread: string;
  path: string;
  /** For a change: the item of the tool call that made it. */
  item: string;
  /** For Git: whether it is what is staged. */
  staged: boolean;
}

export function differenceOf(tab: Tab): DifferenceTabOf | null {
  const of = tab.of;
  if (tab.view !== 'difference' || of?.env === undefined || of.thread === undefined || of.path === undefined) return null;
  return { kind: of.kind === 'git' ? 'git' : 'change', env: of.env, thread: of.thread, path: of.path, item: of.item ?? '', staged: of.staged === '1' };
}

/** Opens the difference of a change, or of a path in Git, as a tab. */
export function openDifference(options: { kind: 'change' | 'git'; environmentId: string; threadId: string; path: string; item?: string; staged?: boolean }): void {
  openContent({ view: 'difference', environmentId: options.environmentId, threadId: options.threadId === '' ? null : options.threadId,
    path: options.path, kind: options.kind, item: options.item ?? '', staged: options.kind === 'git' && options.staged === true }, 'normal', 'push');
}

// ---------------------------------------------------------------- the viewers, fetched when wanted

/** The component of each viewer, fetched when first drawn. */
const SLOTS: Record<ViewerId, ComponentType<ViewerProps>> = Object.fromEntries(VIEWERS.map((viewer) => [viewer.id, lazyPart<ViewerProps>(viewer.load)])) as Record<
  ViewerId,
  ComponentType<ViewerProps>
>;

function ViewerSlot({ viewer, props }: { viewer: Viewer; props: ViewerProps }) {
  const Body = SLOTS[viewer.id];
  return <Body {...props} />;
}

// ---------------------------------------------------------------- the tab

function whyReadOnly(
  mayManage: boolean,
  environment: Environment | undefined,
  environmentName: string,
  asOf: string | null,
  viewer: Viewer,
  isText: boolean,
): ReadOnlyReason | null {
  if (!mayManage) return { kind: 'permission' };
  if (isAway(environment)) return { kind: 'offline', environment: environmentName, asOf };
  if (environment !== undefined && !environment.remote_write) return { kind: 'no_writes', machine: environment.machine?.name ?? null };
  if (!viewer.edits || !isText) return { kind: 'viewer' };
  return null;
}

/** The body of the `file` view: one file of a workspace, in the viewer that takes it. */
export function FileTab({ tab }: ViewProps) {
  const { t } = useTranslation();
  const of = fileOf(tab);
  const userId = useStore(client.session, (state) => state.userId);

  const doorState = useWorkspaceDoor(of === null || of.thread === '' ? null : of.thread, of?.env ?? null);
  const path = of?.path ?? '';
  useTabName(tab.id, baseName(path), FileIcon);

  if (of === null) return <EmptyState compact className="h-full" icon={<FileQuestion aria-hidden />} title={t('files.tab.unknown')} />;
  if (doorState.kind === 'error') return <ErrorState compact className="h-full" error={doorState.error} />;
  if (doorState.kind === 'loading') return <SkeletonRows rows={6} />;
  if (doorState.kind === 'none') return <EmptyState compact className="h-full" icon={<FileQuestion aria-hidden />} title={t('files.tab.noDoor')} />;
  const { access } = doorState;
  if (!access.mayView) {
    const missing = workspaceDenial(t, 'files.view', access.environment);
    return <EmptyState compact className="h-full" icon={<FileQuestion aria-hidden />} title={missing} />;
  }
  return <FileBody key={`${of.env}\u0000${of.path}`} tab={tab} of={of} userId={userId} access={access} />;
}

function FileBody({ tab, of, userId, access }: { tab: Tab; of: FileTabOf; userId: string | null; access: EnvironmentWorkspaceAccess }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const small = useViewportShape() === 'small';
  const queryClient = useQueryClient();
  const { door } = access;
  const { head, error, loading, reload } = useFileHead(door, of.path);
  const { environment } = useEnvironment(door.environmentId);
  const parent = parentOf(of.path);
  const binding = environment === undefined ? null : workspaceBinding(environment);
  const { directories, capacity, tree } = useDirectoryTree(door, parent, binding);
  const directory = directories.get(directoryKey(door.environmentId, parent));
  const [override, setOverride] = useState<ViewerId | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadError, setDownloadError] = useState<unknown>(null);
  const heldJump = useJumps((state) => state.jumps[tab.id]);
  const line = fileLine(tab.of?.line);
  // The persisted address wins over a prior local open, including Back/Forward and no line.
  // Zero denotes address-only intent; explicit repeated requests have positive nonces.
  const jump = line === undefined ? undefined : heldJump?.line === line ? heldJump : { line, nonce: 0 };

  // A wide directory event also covers files beyond its first page. Refresh only this visible
  // exact head once per observation generation; missing partial rows never prove deletion.
  const entry = directory?.entries.find((candidate) => candidate.path === of.path);
  const observed = useRef<{ binding: string; generation: number; stamp: string } | null>(null);
  useEffect(() => {
    if (!shown || head === undefined || binding === null || directory === undefined || directory.page === null && directory.invalidated !== true) return;
    const before = observed.current;
    const stamp = JSON.stringify([entry?.mtime, entry?.size]);
    if (before?.binding === binding && before.generation === directory.generation && before.stamp === stamp) return;
    observed.current = { binding, generation: directory.generation, stamp };
    if (before !== null && (before.binding !== binding || before.generation !== directory.generation)
      || entry !== undefined && (entry.mtime !== head.stat.mtime || entry.size !== head.stat.size)) {
      void invalidateHead(queryClient, door, of.path);
    }
  }, [binding, directory?.generation, directory?.page, directory?.invalidated, entry?.mtime, entry?.size, head?.stat.mtime, head?.stat.size, shown]); // eslint-disable-line react-hooks/exhaustive-deps

  const chosen = useViewerChoice(userId, baseName(of.path));
  const viewer = useMemo<Viewer | null>(() => {
    if (head === undefined) return null;
    const facts = factsOf(head.stat);
    if (override !== null) return VIEWERS.find((candidate) => candidate.id === override) ?? chosenViewer(facts, null);
    return chosenViewer(facts, chosen);
  }, [head, override, chosen]);

  if (directory?.watch === 'refused') return <ErrorState compact className="h-full" error={directory.error} />;

  if (error !== null && head === undefined) {
    const gone = error instanceof ApiError && error.is('not_found');
    return gone ? (
      <EmptyState compact className="h-full" icon={<FileQuestion aria-hidden />} title={t('files.tab.gone', { name: baseName(of.path) })} description={of.path} />
    ) : (
      <ErrorState compact className="h-full" error={error} onRetry={reload} />
    );
  }
  if (loading || head === undefined || viewer === null) return <SkeletonRows rows={8} />;

  const facts = factsOf(head.stat);
  const reason: ReadOnlyReason | null = small ? { kind: 'small_screen' } : whyReadOnly(access.mayManage, environment, access.environment.name, directory?.asOf ?? null, viewer, head.stat.is_text);
  const props: ViewerProps = { door, path: of.path, head, line: jump?.line ?? null, lineRequest: jump?.nonce, tabId: tab.id, whyReadOnly: reason, thread: access.thread, openAs: setOverride };
  const choices = viewersFor(facts);
  const updatesPaused = capacity !== null || directory?.capacity !== undefined && directory.capacity !== null
    || directory?.watch === 'failed' || directory?.invalidated === true;
  const updateReason = capacity !== null || directory?.capacity !== undefined && directory.capacity !== null
    ? t('files.pages.capacity') : directory?.watchError?.detail ?? t('files.tree.changed');

  const download = () => {
    setDownloading(true);
    setDownloadError(null);
    downloadFile(door, of.path, head.stat.name)
      .catch((failure: unknown) => setDownloadError(failure))
      .finally(() => setDownloading(false));
  };

  return (
    <div className="flex h-full min-h-0 flex-col" data-file-tab={of.path} onDragOverCapture={viewerDragOver} onDropCapture={viewerDrop}>
      {of.thread !== '' && door.threadId === null && <p className="shrink-0 px-2 py-1 text-xs text-text-muted" role="status">{t('files.tab.originalWorkspace', { name: access.environment.name })}</p>}
      <div className="flex min-h-(--control-md) items-center gap-1 border-b border-border px-2 text-sm">
        <span className="min-w-0 flex-1 truncate font-mono text-text-muted" title={of.path}>
          {of.path}
        </span>
        <CopyButton value={of.path} label={t('files.copyPath')} />
        {choices.length > 1 && (
          <Menu
            label={t('files.openWith.label')}
            trigger={
              <Button size="sm" variant="ghost" icon={<ScanEye aria-hidden />}>
                {t(viewer.title)}
              </Button>
            }
            entries={[
              {
                type: 'radio',
                id: 'viewer',
                label: t('files.openWith.label'),
                value: viewer.id,
                onValueChange: (value) => {
                  const next = value as ViewerId;
                  setOverride(next);
                  rememberChoice(userId, head.stat.name, next);
                },
                options: choices.map((choice) => ({ value: choice.id, ...choiceOption(t, 'dynamic.value', { name: t(choice.title), context: t('files.openWith.label') }) })),
              },
            ]}
          />
        )}
        <Button size="sm" variant="ghost" icon={<Download aria-hidden />} pending={downloading} onClick={download}>
          {t('files.download')}
        </Button>
      </div>
      {updatesPaused && <InlineAlert tone="warning" className="mx-2 my-1 shrink-0" title={t('files.tab.updatesPaused')}
        action={<Button size="sm" variant="ghost" onClick={() => tree.navigate(parent, 'restart')}>{t('common.tryAgain')}</Button>}>
        {updateReason}
      </InlineAlert>}
      {downloadError !== null && (
        <ErrorState compact error={downloadError} message={t('files.downloadFailed')} />
      )}
      {head.brought.state === 'unavailable' && viewer.id !== 'none' && viewer.id !== 'archive' ? (
        <Unavailable reason={head.brought.reason} />
      ) : (
        <div className="min-h-0 flex-1">
          <ErrorBoundary
            resetKey={viewer.id}
            fallback={(failure, retry) => <ErrorState compact className="h-full" error={failure} title={t('files.viewers.failed')} onRetry={retry} />}
          >
            <Suspense fallback={<SkeletonRows rows={8} />}>
              <ViewerSlot viewer={viewer} props={props} />
            </Suspense>
          </ErrorBoundary>
        </div>
      )}
    </div>
  );
}
