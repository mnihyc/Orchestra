/** Keyboard and open-dialog writes use the same current workspace gate as row menus. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeAll, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import type * as FileContent from '@/features/files/content';
import type * as FileDoors from '@/features/files/door';
import type * as Narrow from '@/features/workbench/Narrow';
import { environmentWorkspaceAccess } from '@/features/files/door';
import { FileTab } from '@/features/files/FileTabs';
import { makeTab } from '@/features/workbench/layout';
import { useFileActions, useFileClipboard } from '@/features/files/FileMenu';
import { client } from '@/api/client';
import { i18n, setLocale } from '@/i18n';
import type { DirEntry, FileRenameResult } from '@/generated/api-types';
import { detailEnvironment } from './resource-detail-fixtures';
import { workspaceFile } from '../doubles/workspace-file';
import { show } from './support';

const calls = vi.hoisted((): { rename: ReturnType<typeof vi.fn>; remove: ReturnType<typeof vi.fn>; copy: ReturnType<typeof vi.fn>; write: ReturnType<typeof vi.fn>; small: boolean; cut: boolean; fileGate: Gate } => ({ rename: vi.fn(), remove: vi.fn(), copy: vi.fn(), write: vi.fn(), small: true, cut: false, fileGate: 'writable' }));
vi.mock('@/api/workspace', async (load) => ({ ...await load<typeof Workspace>(), renameFile: calls.rename, deleteFile: calls.remove, copyFile: calls.copy, writeFile: calls.write,
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
  useFileHead: () => ({ head: { stat: workspaceFile('web/app.txt', 'complete file'), brought: { state: 'ready', hash: 'observed-version', size: 13 } }, error: null, loading: false, reload: vi.fn() }),
  useFileText: () => ({ text: { text: 'complete file', hash: 'observed-version', total: calls.cut ? 3_000_000 : 13, cut: calls.cut }, error: null }),
}));
const file: DirEntry = { path: 'web/app.ts', name: 'app.ts', type: 'file', size: 4, mode: 420, cached: false, mtime: '2026-01-01T00:00:00Z', link_target: null };
const batch = [file, { ...file, path: 'web/other.ts', name: 'other.ts' }];
const nothing = () => undefined;
type Gate = 'writable' | 'permission' | 'offline' | 'policy';
function Probe({ gate, open = nothing }: { gate: Gate; open?: (entry: DirEntry, permanent: boolean) => void }) {
  const environment = { ...detailEnvironment, status: gate === 'offline' ? 'stopped' as const : 'running' as const, machine: null, remote_write: gate !== 'policy' };
  const access = { ...environmentWorkspaceAccess(environment), mayView: true, mayManage: gate !== 'permission' };
  const actions = useFileActions({ access, environment, open, collapseAll: nothing, search: nothing, entries: () => [file] });
  return <><button onClick={() => actions.start({ kind: 'rename', entry: file })}>Rename file</button><button onClick={() => actions.start({ kind: 'delete', entries: [file] })}>Delete file</button><button onClick={() => {
    const paste = actions.emptyEntries('web/out').find((entry) => entry.id === 'paste');
    if (paste !== undefined && 'onSelect' in paste) paste.onSelect();
  }}>Paste files</button><button onClick={() => actions.start({ kind: 'move', entries: batch })}>Move files</button><button onClick={() => actions.move({ ...file, path: 'web/out', name: 'out', type: 'dir' }, batch.map((entry) => entry.path))}>Drop files</button><button onClick={() => actions.start({ kind: 'delete', entries: batch })}>Delete files</button><button onClick={() => actions.start({ kind: 'newFile', directory: 'web' })}>New file</button>{actions.dialogs}</>;
}
afterEach(async () => { cleanup(); vi.clearAllMocks(); calls.fileGate = 'writable'; calls.small = true; calls.cut = false; useFileClipboard.setState({ environmentId: null, mode: null, paths: [] }); await setLocale('en'); });

function FilePane() {
  return <FileTab tab={makeTab('file', { env: detailEnvironment.id, thread: '', path: 'web/app.txt' })} area="main" />;
}

// These cases exercise write gates; load the real lazy module before timing the controls.
beforeAll(async () => { await import('@/features/files/viewers/Code'); });

it('says why a read-only single viewer uses its default without erasing an unavailable saved viewer', async () => {
  const before = client.session.getState(), user = 'single-viewer-reader', key = `orch.${user}.viewers`;
  const saved = JSON.stringify({ '.txt': 'markdown' }); localStorage.setItem(key, saved); calls.fileGate = 'permission';
  client.session.setState({ userId: user, status: 'signed_in', readerVersion: (before.readerVersion ?? 0) + 1 });
  const get = localStorage.getItem.bind(localStorage), reads = vi.fn((stored: string) => get(stored));
  const observed = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(reads);
  const view = show(<FilePane />);
  try {
    expect(await screen.findByRole('textbox', { name: 'Text of app.txt' })).toHaveAttribute('aria-readonly', 'true');
    expect(screen.getByText('Using default · Saved choice unavailable')).toBeVisible();
    const count = reads.mock.calls.filter(([stored]) => stored === key).length;
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    const facts = document.querySelector('[data-fact-disclosure-body]');
    expect(facts).toHaveTextContent(i18n.t('files.viewers.code')); expect(facts).toHaveTextContent(i18n.t('files.viewers.markdown'));
    expect(reads.mock.calls.filter(([stored]) => stored === key)).toHaveLength(count); expect(get(key)).toBe(saved);
    act(() => client.session.setState({ userId: 'replacement-file-reader', readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
    expect(screen.queryByText('Using default · Saved choice unavailable')).not.toBeInTheDocument();
  } finally { view.unmount(); observed.mockRestore(); localStorage.removeItem(key); client.session.setState(before); }
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
  const user = userEvent.setup(); calls.rename.mockResolvedValue({ ...workspaceFile('web/renamed.ts', 'file'), kept_entries: 0 });
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

it.each(['en', 'zh-CN'] as const)('says how many entries a move left safe at its source (%s)', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  calls.rename.mockResolvedValue({ ...workspaceFile('web/renamed.ts', 'copied'), kept_entries: 2 });
  show(<Probe gate="writable" />);
  await user.click(screen.getByRole('button', { name: 'Rename file' }));
  const dialog = within(screen.getByRole('dialog'));
  await user.clear(dialog.getByRole('textbox', { name: i18n.t('files.name.name') }));
  await user.type(dialog.getByRole('textbox', { name: i18n.t('files.name.name') }), 'renamed.ts');
  await user.click(dialog.getByRole('button', { name: i18n.t('files.name.confirm.rename') }));
  expect(await screen.findByText(locale === 'en' ? '2 entries left at web/app.ts' : 'web/app.ts 处保留了 2 个条目')).toBeVisible();
  expect(calls.rename).toHaveBeenCalledWith({ environmentId: detailEnvironment.id, threadId: null }, 'web/app.ts', 'web/renamed.ts');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('keeps a late move notice with the reader who started it', async () => {
  let finish: (value: FileRenameResult) => void = () => undefined;
  const pending = new Promise<FileRenameResult>((resolve) => { finish = resolve; });
  calls.rename.mockReturnValueOnce(pending);
  const user = userEvent.setup();
  show(<Probe gate="writable" />);
  await user.click(screen.getByRole('button', { name: 'Rename file' }));
  const dialog = within(screen.getByRole('dialog'));
  await user.clear(dialog.getByRole('textbox', { name: 'Name' }));
  await user.type(dialog.getByRole('textbox', { name: 'Name' }), 'renamed.ts');
  await user.click(dialog.getByRole('button', { name: 'Rename' }));
  expect(calls.rename).toHaveBeenCalledTimes(1);
  act(() => { client.session.setState((state) => ({ readerVersion: (state.readerVersion ?? 0) + 1 })); });
  await act(async () => { finish({ ...workspaceFile('web/renamed.ts', 'copied'), kept_entries: 9 }); await pending; });
  expect(screen.queryByText('9 entries left at web/app.ts')).not.toBeInTheDocument();
});

it('leaves the previous readers file dialog behind when the reader changes', async () => {
  const user = userEvent.setup(); show(<Probe gate="writable" />);
  await user.click(screen.getByRole('button', { name: 'Rename file' }));
  await user.clear(within(screen.getByRole('dialog')).getByRole('textbox', { name: 'Name' }));
  await user.type(within(screen.getByRole('dialog')).getByRole('textbox', { name: 'Name' }), 'private-draft.ts');
  act(() => { client.session.setState((state) => ({ readerVersion: (state.readerVersion ?? 0) + 1 })); });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Rename file' }));
  expect(within(screen.getByRole('dialog')).getByRole('textbox', { name: 'Name' })).toHaveValue(file.name);
  expect(calls.rename).not.toHaveBeenCalled();
});

it.each(['move', 'drop', 'delete'] as const)('starts no next %s for a different reader', async (kind) => {
  let finish: (value: unknown) => void = () => undefined;
  const pending = new Promise<unknown>((resolve) => { finish = resolve; });
  const sent = kind === 'delete' ? calls.remove : calls.rename;
  sent.mockReturnValueOnce(pending);
  const user = userEvent.setup(); show(<Probe gate="writable" />);
  await user.click(screen.getByRole('button', { name: kind === 'move' ? 'Move files' : kind === 'drop' ? 'Drop files' : 'Delete files' }));
  if (kind === 'move') {
    const dialog = within(screen.getByRole('dialog'));
    await user.clear(dialog.getByRole('textbox', { name: i18n.t('files.name.destination') }));
    await user.type(dialog.getByRole('textbox', { name: i18n.t('files.name.destination') }), 'web/out');
    await user.click(dialog.getByRole('button', { name: i18n.t('files.name.confirm.move') }));
  } else if (kind === 'delete') {
    await user.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: i18n.t('files.delete.confirm') }));
  }
  expect(sent).toHaveBeenCalledTimes(1);
  act(() => { client.session.setState((state) => ({ readerVersion: (state.readerVersion ?? 0) + 1 })); });
  await act(async () => { finish(kind === 'delete' ? undefined : { ...workspaceFile('web/out/app.ts', 'copied'), kept_entries: 0 }); await pending; });
  expect(sent).toHaveBeenCalledTimes(1);
});

it('opens no newly written file for a different reader', async () => {
  let finish: (value: unknown) => void = () => undefined;
  const pending = new Promise<unknown>((resolve) => { finish = resolve; });
  calls.write.mockReturnValueOnce(pending);
  const opened = vi.fn(); const user = userEvent.setup(); show(<Probe gate="writable" open={opened} />);
  await user.click(screen.getByRole('button', { name: 'New file' }));
  const dialog = within(screen.getByRole('dialog'));
  await user.type(dialog.getByRole('textbox', { name: 'Name' }), 'private-draft.ts');
  await user.click(dialog.getByRole('button', { name: i18n.t('files.name.confirm.create') }));
  expect(calls.write).toHaveBeenCalledTimes(1);
  act(() => { client.session.setState((state) => ({ readerVersion: (state.readerVersion ?? 0) + 1 })); });
  await user.click(screen.getByRole('button', { name: 'Rename file' }));
  await user.clear(within(screen.getByRole('dialog')).getByRole('textbox', { name: 'Name' }));
  await user.type(within(screen.getByRole('dialog')).getByRole('textbox', { name: 'Name' }), 'new-reader.ts');
  await act(async () => { finish(workspaceFile('web/private-draft.ts', '')); await pending; });
  expect(opened).not.toHaveBeenCalled();
  expect(within(screen.getByRole('dialog')).getByRole('textbox', { name: 'Name' })).toHaveValue('new-reader.ts');
});
