/** Exact-turn Review keeps all five file acts and performs guarded text Undo only after confirmation. */
import type { ReactNode } from 'react';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import { ApiError } from '@/api/client';
import type { WorkspaceAccess } from '@/features/files/door';
import { ChangesView } from '@/features/inspector/Changes';
import { UndoChange } from '@/features/timeline/UndoChange';
import { useComposerStore } from '@/features/composer/Draft';
import type { EnvironmentChange, GitStatus } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { workspaceFile, workspaceGit } from '../doubles/workspace-file';
import { AT, change } from '../doubles/thread-rows';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const wire = vi.hoisted(() => ({ access: null as WorkspaceAccess | null, rows: [] as EnvironmentChange[], git: null as GitStatus | null, gitRead: vi.fn(), text: 'new\n', read: vi.fn(), file: vi.fn(), difference: vi.fn(), copy: vi.fn(), write: vi.fn<typeof Workspace.writeFile>(), stat: vi.fn<typeof Workspace.statFile>() }));
vi.mock('@/features/inspector/parts', async (original) => ({ ...await original<object>(), ThreadViewFrame: ({ children }: { children: (access: WorkspaceAccess) => ReactNode }) => wire.access === null ? null : children(wire.access) }));
vi.mock('@/features/files/FileTabs', async (original) => ({ ...await original<object>(), openFile: wire.file, openDifference: wire.difference }));
vi.mock('@/ui/clipboard', async (original) => ({ ...await original<object>(), copyText: wire.copy }));
vi.mock('@/api/workspace', async (original) => {
  const actual = await original<typeof Workspace>();
  return { ...actual, changesQuery: (door: Workspace.Door, all: boolean, turnId?: string) => ({ ...actual.changesQuery(door, all, turnId), queryFn: () => { wire.read(door, all, turnId); return Promise.resolve(wire.rows); } }),
    gitStatusQuery: (door: Workspace.Door) => ({ ...actual.gitStatusQuery(door), queryFn: () => { wire.gitRead(door); if (wire.git === null) throw new Error('The native repository report is required'); return Promise.resolve(wire.git); } }),
    statFile: wire.stat, fetchFile: () => Promise.resolve({ state: 'ready', hash: 'observed-version', size: new TextEncoder().encode(wire.text).length }),
    readFileBytes: () => Promise.resolve({ bytes: new TextEncoder().encode(wire.text), total: new TextEncoder().encode(wire.text).length }), writeFile: wire.write };
});
afterEach(async () => { cleanup(); vi.resetAllMocks(); useComposerStore.setState({ text: {} }); await setLocale('en'); });
function fixture() {
  const thread = snapshot(null).thread; const environment = thread.environment;
  if (environment === null) throw new Error('Review needs its native workspace');
  const access: WorkspaceAccess = { thread, environment, directory: thread.directory, door: { environmentId: environment.id, threadId: thread.id }, mayView: true, mayManage: true, mayWatchProcesses: true, mayShell: true };
  const record: EnvironmentChange = { ...change('web/src/a.ts', '@@ -1 +1 @@\n-old\n+new\n'), item_id: 'change-item', thread_id: thread.id, at: AT };
  wire.access = access; wire.rows = [record]; wire.git = workspaceGit(); wire.text = 'new\n'; wire.copy.mockResolvedValue(true);
  wire.stat.mockResolvedValue(workspaceFile(record.path, wire.text)); wire.write.mockResolvedValue(workspaceFile(record.path, 'old\n', { hash: 'after-undo' }));
  return { access, record };
}

it.each(['en', 'zh-CN'] as const)('reads the exact turn and runs Open, Difference, Copy patch, Add to message and Undo in %s', async (locale) => {
  await setLocale(locale); const { access, record } = fixture();
  show(<ChangesView tab={{ id: 'review-panel', view: 'changes', of: { thread: access.thread.id, turn: 'exact-turn', scope: 'turn' } }} area="main" />);
  await screen.findByText('a.ts');
  expect(wire.read).toHaveBeenCalledWith(access.door, false, 'exact-turn');
  expect(wire.gitRead).not.toHaveBeenCalled();
  expect(wire.stat).not.toHaveBeenCalled(); expect(wire.write).not.toHaveBeenCalled();
  const choose = async (name: string) => { await userEvent.click(screen.getByRole('button', { name: i18n.t('inspector.changes.menuFor', { path: record.path }) })); await userEvent.click(await screen.findByRole('menuitem', { name })); };
  await choose(i18n.t('inspector.changes.openFile'));
  expect(wire.file).toHaveBeenCalledExactlyOnceWith({ environmentId: access.environment.id, threadId: access.thread.id, path: record.path, permanent: false });
  await choose(i18n.t('inspector.changes.openDifference'));
  expect(wire.difference).toHaveBeenCalledExactlyOnceWith({ kind: 'change', environmentId: access.environment.id, threadId: access.thread.id, path: record.path, item: record.item_id });
  await choose(i18n.t('timeline.step.copyPatch')); expect(wire.copy).toHaveBeenCalledExactlyOnceWith(record.diff);
  await choose(i18n.t('files.addToMessage')); expect(useComposerStore.getState().text[access.thread.id]).toContain(record.diff);
  await choose(i18n.t('thread.undo.action'));
  const confirm = await screen.findByRole('button', { name: i18n.t('thread.undo.confirm') });
  await waitFor(() => expect(confirm).toBeEnabled());
  expect(wire.write).not.toHaveBeenCalled();
  await userEvent.click(confirm);
  await waitFor(() => expect(wire.write).toHaveBeenCalledExactlyOnceWith(access.door, record.path, 'old\n', 'observed-version'));
});

it.each(['en', 'zh-CN'] as const)('selects native turn, whole-thread and Git scopes with no eager repository or duplicate changes read in %s', async (locale) => {
  await setLocale(locale); const { access } = fixture();
  const view = show(<ChangesView tab={{ id: 'scopes-panel', view: 'changes', of: { thread: access.thread.id, turn: 'exact-turn', scope: 'turn' } }} area="main" />);
  await screen.findByText('a.ts'); expect(wire.gitRead).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('thread.changeScopes.label') }));
  await userEvent.click(await screen.findByRole('option', { name: i18n.t('thread.changeScopes.thread') }));
  await waitFor(() => expect(wire.read).toHaveBeenCalledWith(access.door, false, undefined));
  expect(wire.gitRead).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('thread.changeScopes.label') }));
  await userEvent.click(await screen.findByRole('option', { name: i18n.t('thread.changeScopes.git') }));
  expect(await screen.findByText(i18n.t('inspector.git.clean'))).toBeVisible();
  expect(view.container.querySelector('[data-testid="git-panel"]')).not.toBeNull();
  expect(wire.gitRead).toHaveBeenCalledTimes(1);
});

it.each(['en', 'zh-CN'] as const)('omits a Git scope for a reported non-repository and explains it by hover in %s', async (locale) => {
  await setLocale(locale); const { access } = fixture(); wire.git = workspaceGit({ available: false, reason: 'not_a_repository', branch: null, head: null, repository_dir: null, status_hash: null });
  show(<ChangesView tab={{ id: 'no-repository', view: 'changes', of: { thread: access.thread.id } }} area="main" />);
  await screen.findByText('a.ts');
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('thread.changeScopes.label') }));
  await waitFor(() => expect(wire.gitRead).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole('option', { name: i18n.t('thread.changeScopes.git') })).not.toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  const reason = await screen.findByRole('img', { name: i18n.t('inspector.git.needsRepository') });
  await userEvent.hover(reason); expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('inspector.git.needsRepository'));
});

it('refuses newer touched lines before a write and leaves a server hash conflict visible with a fresh-plan action', async () => {
  const { access, record } = fixture(); wire.text = 'changed again\n';
  const view = show(<UndoChange change={record} access={access} onClose={vi.fn()} />);
  expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('thread.undo.changed'));
  expect(screen.getByRole('button', { name: i18n.t('thread.undo.confirm') })).toBeDisabled(); expect(wire.write).not.toHaveBeenCalled();
  view.unmount(); wire.text = 'new\n';
  wire.write.mockRejectedValue(new ApiError({ code: 'conflict.stale_revision', type: 'orch:error:conflict.stale_revision', status: 409, title: 'Changed', detail: 'The file changed after the preview.', request_id: 'synthetic' }, null));
  show(<UndoChange change={record} access={access} onClose={vi.fn()} />);
  const confirm = screen.getByRole('button', { name: i18n.t('thread.undo.confirm') }); await waitFor(() => expect(confirm).toBeEnabled());
  await userEvent.click(confirm);
  expect(await screen.findByRole('button', { name: i18n.t('source.restore.freshPlan') })).toBeVisible();
  expect(confirm).toBeDisabled(); expect(wire.write).toHaveBeenCalledTimes(1);
});
