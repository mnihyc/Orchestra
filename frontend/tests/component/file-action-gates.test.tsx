/** Keyboard and open-dialog writes use the same current workspace gate as row menus. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import { environmentWorkspaceAccess } from '@/features/files/door';
import { useFileActions, useFileClipboard } from '@/features/files/FileMenu';
import { client } from '@/api/client';
import type { DirEntry } from '@/generated/api-types';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ rename: vi.fn(), remove: vi.fn(), copy: vi.fn() }));
vi.mock('@/api/workspace', async (load) => ({ ...await load<typeof Workspace>(), renameFile: calls.rename, deleteFile: calls.remove, copyFile: calls.copy,
  listDirectory: () => Promise.resolve({ entries: [], path: 'web', mount: null, source: 'live', as_of: '2026-01-01T00:00:00Z', truncated: false }),
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
afterEach(() => { cleanup(); vi.clearAllMocks(); useFileClipboard.setState({ environmentId: null, mode: null, paths: [] }); });

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
