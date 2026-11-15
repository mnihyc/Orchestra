/** Keyboard and open-dialog writes use the same current workspace gate as row menus. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeAll, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import type * as FileContent from '@/features/files/content';
import type * as FileDoors from '@/features/files/door';
import type * as Narrow from '@/features/workbench/Narrow';
import { environmentWorkspaceAccess } from '@/features/files/door';
import { FileTab } from '@/features/files/FileTabs';
import { WorkspaceInstructions } from '@/features/start/WorkspaceInstructions';
import { useFileHead } from '@/features/files/content';
import { makeTab } from '@/features/workbench/layout';
import { useFileActions, useFileClipboard } from '@/features/files/FileMenu';
import { ApiError, client } from '@/api/client';
import { writeFile } from '@/api/workspace';
import { useWorkbench } from '@/features/workbench/store';
import { EditorView } from '@codemirror/view';
import { QueryClient } from '@tanstack/react-query';
import { forgetUnsaved, readUnsaved } from '@/features/files/Editing';
import type * as EnvironmentsApi from '@/api/environments';
import type { DirEntry, Environment } from '@/generated/api-types';
import { detailEnvironment } from './resource-detail-fixtures';
import { workspaceFile } from '../doubles/workspace-file';
import { show } from './support';

const calls = vi.hoisted((): { rename: ReturnType<typeof vi.fn>; remove: ReturnType<typeof vi.fn>; copy: ReturnType<typeof vi.fn>; environment: ReturnType<typeof vi.fn<(id: string) => Promise<Environment>>>; headError: unknown; small: boolean; cut: boolean; fileGate: Gate } => ({ rename: vi.fn(), remove: vi.fn(), copy: vi.fn(), environment: vi.fn<(id: string) => Promise<Environment>>(), headError: null, small: true, cut: false, fileGate: 'writable' }));
vi.mock('@/api/environments', async (load) => ({ ...await load<typeof EnvironmentsApi>(),
  environmentQuery: (id: string) => ({ queryKey: ['environment', id], queryFn: () => calls.environment(id) }),
}));
vi.mock('@/api/workspace', async (load) => ({ ...await load<typeof Workspace>(), renameFile: calls.rename, deleteFile: calls.remove, copyFile: calls.copy,
  writeFile: vi.fn(),
  listDirectory: () => Promise.resolve({ entries: [], path: 'web', mount: null, source: 'live', as_of: '2026-01-01T00:00:00Z', truncated: false }),
}));
vi.mock('@/features/workbench/Narrow', async (load) => ({ ...await load<typeof Narrow>(), useViewportShape: () => calls.small ? 'small' : 'wide' }));
vi.mock('@/features/files/door', async (load) => {
  const actual = await load<typeof FileDoors>();
  const environment = () => ({ ...detailEnvironment, status: calls.fileGate === 'offline' ? 'stopped' as const : 'running' as const,
    remote_write: calls.fileGate !== 'policy', machine: null });
  return { ...actual, useEnvironment: () => ({ environment: environment(), failed: null }),
    useWorkspaceDoor: () => ({ kind: 'ready', access: { ...actual.environmentWorkspaceAccess(environment()), mayView: true, mayManage: calls.fileGate !== 'permission' } }),
  };
});
vi.mock('@/features/files/useDirectoryTree', () => ({ useDirectoryTree: () => ({ directories: new Map(), capacity: null, tree: new Map() }) }));
vi.mock('@/features/files/content', async (load) => ({ ...await load<typeof FileContent>(),
  useFileHead: vi.fn((_door: Workspace.Door, path: string) => ({ head: { stat: workspaceFile(path, 'complete file'), brought: { state: 'ready', hash: 'observed-version', size: 13 } }, error: calls.headError, loading: false, reload: vi.fn() })),
  useFileText: () => ({ text: { text: 'complete file', hash: 'observed-version', total: calls.cut ? 3_000_000 : 13, cut: calls.cut }, error: null }),
}));
const file: DirEntry = { path: 'web/app.ts', name: 'app.ts', type: 'file', size: 4, mode: 420, cached: false, mtime: '2026-01-01T00:00:00Z', link_target: null };
const nothing = () => undefined;
type Gate = 'writable' | 'permission' | 'offline' | 'policy';
function Probe({ gate }: { gate: Gate }) {
  const environment = { ...detailEnvironment, status: gate === 'offline' ? 'stopped' as const : 'running' as const, machine: null, remote_write: gate !== 'policy' };
  const access = { ...environmentWorkspaceAccess(environment), mayView: true, mayManage: gate !== 'permission' };
  const actions = useFileActions({ access, environment, open: nothing, collapseAll: nothing, search: nothing, entries: () => [file] });
  return <><button onClick={() => actions.start({ kind: 'rename', entry: file })}>Rename file</button><button onClick={() => actions.start({ kind: 'delete', entries: [file] })}>Delete file</button><button onClick={() => {
    const paste = actions.emptyEntries('web/out').find((entry) => entry.id === 'paste');
    if (paste !== undefined && 'onSelect' in paste) paste.onSelect();
  }}>Paste files</button>{actions.dialogs}</>;
}
afterEach(() => { cleanup(); vi.clearAllMocks(); calls.fileGate = 'writable'; calls.small = true; calls.cut = false; calls.headError = null; useFileClipboard.setState({ environmentId: null, mode: null, paths: [] }); });

function FilePane() {
  return <FileTab tab={makeTab('file', { env: detailEnvironment.id, thread: '', path: 'web/app.txt' })} area="main" />;
}

// These cases exercise write gates; load the real lazy module before timing the controls.
beforeAll(async () => { await import('@/features/files/viewers/Code'); });

it('edits and version-checks a shared workspace file outside a tab without marking the conversation unsaved', async () => {
  const CodeViewer = (await import('@/features/files/viewers/Code')).default;
  const door = { environmentId: detailEnvironment.id, threadId: null };
  const stat = workspaceFile('web/AGENTS.md', 'complete file');
  vi.mocked(writeFile).mockResolvedValue({ ...stat, hash: 'saved-version' });
  const unsaved = useWorkbench.getState().unsaved, layout = useWorkbench.getState().layout;
  show(<CodeViewer door={door} path={stat.path} head={{ stat, brought: { state: 'ready', hash: 'observed-version', size: 13 } }}
    line={null} whyReadOnly={null} thread={null} openAs={nothing} />);
  const body = await screen.findByRole('textbox', { name: 'Text of AGENTS.md' });
  const editor = EditorView.findFromDOM(body);
  if (editor === null) throw new Error('The actual file editor is mounted');
  act(() => editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: 'Shared workspace instructions' } }));
  expect(body).toHaveTextContent('Shared workspace instructions');
  expect(useWorkbench.getState().unsaved).toBe(unsaved);
  expect(useWorkbench.getState().layout).toBe(layout);
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(writeFile).toHaveBeenCalledExactlyOnceWith(door, 'web/AGENTS.md', 'Shared workspace instructions', 'observed-version'));
  expect(useWorkbench.getState().unsaved).toBe(unsaved);
});

it('keeps the last unsaved file edit when a setup editor closes before its typing pause', async () => {
  const CodeViewer = (await import('@/features/files/viewers/Code')).default;
  const savedSession = client.session.getState();
  const user = 'workspace-editor-reader', path = 'web/AGENTS.md';
  const stat = workspaceFile(path, 'complete file');
  client.session.setState({ userId: user });
  forgetUnsaved(user, detailEnvironment.id, path);
  try {
    const view = show(<CodeViewer door={{ environmentId: detailEnvironment.id, threadId: null }} path={path}
      head={{ stat, brought: { state: 'ready', hash: 'observed-version', size: 13 } }} line={null}
      whyReadOnly={null} thread={null} openAs={nothing} />);
    const body = await screen.findByRole('textbox', { name: 'Text of AGENTS.md' });
    const editor = EditorView.findFromDOM(body);
    if (editor === null) throw new Error('The shared file editor is mounted');
    act(() => editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: 'Keep this unfinished file edit' } }));
    view.unmount();
    expect(readUnsaved(user, detailEnvironment.id, path)).toMatchObject({ base: 'observed-version', text: 'Keep this unfinished file edit' });
    expect(writeFile).not.toHaveBeenCalled();
  } finally {
    forgetUnsaved(user, detailEnvironment.id, path);
    client.session.setState(savedSession);
  }
});

it('opens the selected directory shared instructions in place only when asked', async () => {
  const environment = { ...detailEnvironment, status: 'running' as const, remote_write: true, machine: null,
    access: { ...detailEnvironment.access, permissions: ['files.view', 'files.manage'] } };
  show(<WorkspaceInstructions environment={environment} directory="web" />);
  expect(useFileHead).not.toHaveBeenCalled();
  await userEvent.click(screen.getByText('AGENTS.md', { exact: true }));
  expect(await screen.findByRole('textbox', { name: 'Text of AGENTS.md' })).toHaveAttribute('contenteditable', 'true');
  expect(useFileHead).toHaveBeenCalledWith({ environmentId: environment.id, threadId: null }, 'web/AGENTS.md');
  expect(screen.getByRole('button', { name: 'Save' })).toBeVisible();
});

it('keeps the shared instructions readable without offering a write to a file reader', async () => {
  const environment = { ...detailEnvironment, status: 'running' as const, remote_write: true, machine: null,
    access: { ...detailEnvironment.access, permissions: ['files.view'] } };
  show(<WorkspaceInstructions environment={environment} directory="web" />);
  await userEvent.click(screen.getByText('AGENTS.md', { exact: true }));
  expect(await screen.findByRole('textbox', { name: 'Text of AGENTS.md' })).toHaveAttribute('aria-readonly', 'true');
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  expect(writeFile).not.toHaveBeenCalled();
});

it('removes previously shown instructions when their environment read is refused again', async () => {
  const environment = { ...detailEnvironment, status: 'running' as const, remote_write: true, machine: null,
    access: { ...detailEnvironment.access, permissions: ['files.view', 'files.manage'] } };
  calls.environment.mockResolvedValue(environment);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  show(<WorkspaceInstructions environmentId={environment.id} directory="web" />, cache);
  expect(calls.environment).not.toHaveBeenCalled();
  await userEvent.click(screen.getByText('AGENTS.md', { exact: true }));
  await screen.findByRole('textbox', { name: 'Text of AGENTS.md' });
  calls.environment.mockRejectedValue(new ApiError({ type: 'about:blank', title: 'Access changed', status: 403,
    code: 'authz.denied', detail: 'The environment read is no longer allowed', request_id: 'synthetic' }, null));
  await act(async () => { await cache.invalidateQueries({ queryKey: ['environment', environment.id], exact: true }); });
  await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Text of AGENTS.md' })).not.toBeInTheDocument());
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  expect(writeFile).not.toHaveBeenCalled();
});

it.each([403, 404])('does not keep old instruction text visible after its exact file read returns %s', async (status) => {
  const environment = { ...detailEnvironment, status: 'running' as const, remote_write: true, machine: null,
    access: { ...detailEnvironment.access, permissions: ['files.view', 'files.manage'] } };
  const view = show(<WorkspaceInstructions environment={environment} directory="web" />);
  await userEvent.click(screen.getByText('AGENTS.md', { exact: true }));
  await screen.findByRole('textbox', { name: 'Text of AGENTS.md' });
  calls.headError = new ApiError({ type: 'about:blank', title: 'File unavailable', status,
    code: status === 403 ? 'authz.denied' : 'not_found', detail: 'The file is unavailable', request_id: 'synthetic' }, null);
  view.rerender(<WorkspaceInstructions environment={environment} directory="web" />);
  await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Text of AGENTS.md' })).not.toBeInTheDocument());
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  expect(writeFile).not.toHaveBeenCalled();
});

it.each([true, false])('keeps writable full-text editing and compact Save available on a small screen (%s)', async (small) => {
  calls.small = small;
  show(<FilePane />);
  expect(await screen.findByRole('textbox', { name: 'Text of app.txt' })).toHaveAttribute('contenteditable', 'true');
  expect(screen.getByRole('button', { name: 'Save' })).toBeVisible();
  expect(screen.queryByTestId('read-only-note')).not.toBeInTheDocument();
});

it.each(['permission', 'offline', 'policy', 'partial'] as const)('keeps the actual %s write protection on a narrow file view', async (gate) => {
  calls.fileGate = gate === 'partial' ? 'writable' : gate; calls.cut = gate === 'partial';
  show(<FilePane />);
  expect(await screen.findByRole('textbox', { name: 'Text of app.txt' })).toHaveAttribute('aria-readonly', 'true');
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  expect(screen.getByTestId('read-only-note')).toBeVisible();
});

it.each(['permission', 'offline', 'policy'] as const)('opens no mutation dialog under the %s gate', async (gate) => {
  const user = userEvent.setup(); show(<Probe gate={gate} />);
  await user.click(screen.getByRole('button', { name: 'Rename file' }));
  await user.click(screen.getByRole('button', { name: 'Delete file' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(calls.rename).not.toHaveBeenCalled(); expect(calls.remove).not.toHaveBeenCalled();
});

it('retains the entered name but refuses submit after write access changes', async () => {
  const user = userEvent.setup(); calls.rename.mockResolvedValue(undefined);
  const view = show(<Probe gate="writable" />);
  await user.click(screen.getByRole('button', { name: 'Rename file' }));
  const name = within(screen.getByRole('dialog')).getByRole('textbox', { name: 'Name' });
  await user.clear(name); await user.type(name, 'renamed.ts'); view.rerender(<Probe gate="permission" />);
  expect(name).toHaveValue('renamed.ts');
  expect(within(screen.getByRole('dialog')).getByRole('button', { name: 'Rename' })).toBeDisabled();
  await user.keyboard('{Enter}'); expect(calls.rename).not.toHaveBeenCalled();
  view.rerender(<Probe gate="writable" />);
  await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Rename' }));
  expect(calls.rename).toHaveBeenCalledWith({ environmentId: detailEnvironment.id, threadId: null }, 'web/app.ts', 'web/renamed.ts');
  expect(calls.rename).toHaveBeenCalledTimes(1);
});

it('cannot interpret a clipboard path in another workspace or carry it across readers', async () => {
  const user = userEvent.setup(); show(<Probe gate="writable" />);
  useFileClipboard.setState({ environmentId: 'another-workspace', mode: 'copy', paths: ['web/app.ts'] });
  await user.click(screen.getByRole('button', { name: 'Paste files' }));
  expect(calls.copy).not.toHaveBeenCalled(); expect(calls.rename).not.toHaveBeenCalled();
  client.session.setState((state) => ({ readerVersion: (state.readerVersion ?? 0) + 1 }));
  expect(useFileClipboard.getState()).toEqual({ environmentId: null, mode: null, paths: [] });
  await user.click(screen.getByRole('button', { name: 'Paste files' })); expect(calls.copy).not.toHaveBeenCalled();
});
