/** Remote search reads the successful native Git snapshot and preserves the explicit push/job boundary. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Forks from '@/api/forks';
import type * as Source from '@/api/source';
import type { WorkspaceAccess } from '@/features/files/door';
import { Publishing } from '@/features/source/Publish';
import type { GitStatus, Job } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME, id } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ push: vi.fn<typeof Source.pushBranch>(), job: vi.fn<() => Promise<Job>>() }));
vi.mock('@/api/source', async (load) => ({ ...await load<typeof Source>(), pushBranch: calls.push }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof Forks>(), jobQuery: (id: string) => ({ queryKey: ['metadata-git-job', id], queryFn: calls.job }) }));

function access(): WorkspaceAccess {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The existing native-shaped workspace fixture is missing');
  const thread = world.threads.thread(person, row);
  if (thread.environment === null) throw new Error('The native thread fixture has no environment');
  return { thread, environment: thread.environment, door: { environmentId: thread.environment.id, threadId: thread.id }, directory: 'repo', mayView: true,
    mayManage: thread.environment_permissions.includes('files.manage'), mayWatchProcesses: thread.environment_permissions.includes('processes.view'), mayShell: thread.environment_permissions.includes('terminal.use') };
}

beforeEach(async () => { await setLocale('en'); vi.clearAllMocks(); });
afterEach(() => { cleanup(); });

it('retains an unavailable current remote and pushes a locally found late remote with exact head/url/branch only after Push', async () => {
  const workspace = access(); expect(workspace.mayManage).toBe(true);
  const status: GitStatus = { available: true, reason: null, repository_dir: 'repo', branch: 'main', head: 'a'.repeat(40), status_hash: 'b'.repeat(64), ahead: 1, behind: 0,
    upstream: 'remote-13/main', changes: [], remotes: Array.from({ length: 13 }, (_, index) => ({ name: `remote-${index + 1}`, url: `https://git.example.test/team/repository-${index + 1}.git` })) };
  const job: Job = { id: id(73, 1), kind: 'git_push', environment_id: workspace.environment.id, node_id: workspace.thread.id, provider_id: null, batch_id: null,
    state: 'queued', result: null, error: null, log: null, created_by: workspace.thread.owner, created_at: '2026-10-18T00:00:00Z', started_at: null, finished_at: null,
    progress: { phase: 'push', percent: 0, phases: [], files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, rate_bps: null, eta_seconds: null, message: null } };
  calls.push.mockResolvedValue(job); calls.job.mockResolvedValue(job);
  const user = userEvent.setup(), view = show(<Publishing access={workspace} status={status} />);
  await user.click(screen.getByRole('button', { name: i18n.t('source.git.push') }));
  const dialog = within(screen.getByRole('dialog', { name: i18n.t('source.git.pushTitle') }));
  const picker = dialog.getByRole('combobox', { name: i18n.t('source.git.remote') });
  const branch = dialog.getByRole('textbox', { name: i18n.t('source.git.branch') });
  // Dialog autofocus opens the locator's empty query; leave it to read the current remote.
  await user.click(branch);
  expect(picker).toHaveValue('remote-13');
  await user.click(picker); expect(screen.getAllByRole('option')).toHaveLength(13);
  await user.type(picker, 'repository-12.git');
  expect(screen.getByText('https://git.example.test/team/repository-12.git', { exact: true })).toBeVisible();
  expect(calls.push).not.toHaveBeenCalled(); expect(calls.job).not.toHaveBeenCalled();
  await user.keyboard('{Escape}'); expect(picker).toHaveValue('remote-13');
  view.rerender(<Publishing access={workspace} status={{ ...status, remotes: status.remotes.slice(0, 12) }} />);
  expect(picker).toHaveValue('remote-13'); expect(dialog.getByRole('button', { name: i18n.t('source.git.pushDo') })).toBeDisabled();
  await user.click(picker); await user.type(picker, 'remote-13'); expect(screen.queryByRole('option')).toBeNull();
  await user.keyboard('{Enter}{Escape}'); expect(calls.push).not.toHaveBeenCalled();
  await user.click(picker); await user.type(picker, 'repository-12.git'); await user.keyboard('{Enter}');
  expect(picker).toHaveValue('remote-12'); expect(calls.push).not.toHaveBeenCalled();
  await user.clear(branch); await user.type(branch, ' release/topic ');
  await user.click(dialog.getByRole('button', { name: i18n.t('source.git.pushDo') }));
  await waitFor(() => expect(calls.push).toHaveBeenCalledWith(workspace.door, { path: 'repo', remote: 'remote-12', expected_remote_url: 'https://git.example.test/team/repository-12.git',
    branch: 'release/topic', expected_head: status.head, set_upstream: false }, expect.any(String)));
  expect(calls.push).toHaveBeenCalledOnce(); await waitFor(() => expect(picker).toBeDisabled());
  expect(dialog.queryByRole('button', { name: i18n.t('source.git.pushDo') })).toBeNull();
  await waitFor(() => expect(calls.job).toHaveBeenCalledOnce());
});
