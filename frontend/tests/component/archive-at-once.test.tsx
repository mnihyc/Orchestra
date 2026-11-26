/** An archive is made as soon as its plan is read; only work it stops is said first, in one line, with the threads on Show. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { applyArchivePlan, previewArchive, readNode } from '@/api/nodes';
import { NodeDialogHost } from '@/features/navigation/NodeDialogHost';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { ArchivePlaces } from '@/features/project/PlaceActs';
import type { Node, NodeArchivePlan, Topic } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { ACME, acmeTree } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), readNode: vi.fn(), previewArchive: vi.fn(), applyArchivePlan: vi.fn() }));

const place = { id: ACME.upgradeDeps, kind: 'topic' as const, name: 'Upgrade deps', parentId: ACME.web, archived: false, status: null, folder: false };
const thread = { id: 'thread-running', kind: 'thread' as const, name: 'Bump the lockfile', redacted: false };
function plan(id: string, turns: number): NodeArchivePlan {
  return { preflight_id: `review-${id}`, expires_at: '2099-01-01T00:00:00Z', stamp: `stamp-${id}`, archiving: true, node: { id, kind: 'topic', name: id, redacted: false }, revision: 1,
    subtree: [], counts: { orgs: 0, projects: 0, topics: 1, threads: 1, documents: 0 }, environments: [], copies_kept: 0, live_shares_kept: 0, blockers: [], retention: null,
    work: [{ thread, turns, inputs: 0, approvals: 0, leases: 0, session: false, links: 0 }] } as unknown as NodeArchivePlan;
}
const undo = { ...plan(ACME.upgradeDeps, 0), archiving: false };

beforeEach(() => {
  vi.clearAllMocks();
  treeWriter.reset(ACME.org, acmeTree(), 100);
  vi.mocked(readNode).mockResolvedValue({ id: place.id, kind: 'topic', name: place.name, org_id: ACME.org, revision: 1, archived: false, path: [], access: { permissions: ['topic.manage'] } } as unknown as Node);
  vi.mocked(applyArchivePlan).mockImplementation((reviewed) => Promise.resolve({ node: { id: reviewed.node.id } as unknown as Node, undo: reviewed.archiving ? undo : null }));
});
afterEach(async () => { cleanup(); useTreeStore.setState({ byOrg: new Map() }); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`archives a place that stops nothing at once, with Undo in the notice (${locale})`, async () => {
    await setLocale(locale);
    vi.mocked(previewArchive).mockResolvedValue(plan(ACME.upgradeDeps, 0));
    show(<NodeDialogHost />);
    act(() => openNodeDialog({ type: 'archive', node: place }));
    await waitFor(() => expect(applyArchivePlan).toHaveBeenCalledExactlyOnceWith(plan(ACME.upgradeDeps, 0)));
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('places.toast.undo') }));
    await waitFor(() => expect(applyArchivePlan).toHaveBeenLastCalledWith(undo));
  });
}

it('says first in one line that work running in a thread stops, with the thread on Show, and archives on Archive', async () => {
  vi.mocked(previewArchive).mockResolvedValue(plan(ACME.upgradeDeps, 2));
  show(<NodeDialogHost />);
  act(() => openNodeDialog({ type: 'archive', node: place }));
  const dialog = await screen.findByRole('alertdialog');
  expect(screen.getByText(i18n.t('places.archive.stops', { count: 1 }))).toBeVisible();
  expect(applyArchivePlan).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('actions.show.label') }));
  expect(screen.getByText('Bump the lockfile')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('places.archive.confirm', { name: place.name }) }));
  await waitFor(() => expect(applyArchivePlan).toHaveBeenCalledOnce());
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
});

it('archives selected topics that stop nothing at once and names a refusal by its row', async () => {
  const topics = [{ id: 'one', name: 'Ship it', revision: 1 }, { id: 'two', name: 'Write notes', revision: 1 }] as unknown as Topic[];
  vi.mocked(previewArchive).mockImplementation((id) => Promise.resolve(plan(id, 0)));
  vi.mocked(applyArchivePlan).mockImplementation((reviewed) => reviewed.node.id === 'two' ? Promise.reject(new Error('This topic changed.')) : Promise.resolve({ node: { id: reviewed.node.id } as unknown as Node, undo }));
  const applied = vi.fn();
  const close = vi.fn();
  show(<ArchivePlaces rows={topics} archiving onClose={close} onApplied={applied} />);
  await waitFor(() => expect(applyArchivePlan).toHaveBeenCalledTimes(2));
  const results = await screen.findByRole('dialog', { name: i18n.t('day100.batch.results', { count: 2 }) });
  expect(results).toHaveTextContent('Write notes');
  expect(results).not.toHaveTextContent('Ship it');
  expect(applied).toHaveBeenCalledOnce();
  expect(await screen.findByText(i18n.t('day100.batch.archivedMany', { count: 1 }))).toBeVisible();
});
