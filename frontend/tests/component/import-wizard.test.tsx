/** The wizard carries selected files and reviewed choices into one resumable background import. */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { ImportDialog } from '@/features/people/ImportDialog';
import type { ImportPlan, ImportRequest, Job } from '@/generated/api-types';
import { me, memberAccess, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ plan: vi.fn(), start: vi.fn(), watch: vi.fn(), uploadFails: false }));
vi.mock('@/api/imports', async (original) => ({ ...await original<object>(), planImport: calls.plan, startImport: calls.start }));
vi.mock('@/features/people/importJobs', async (original) => ({ ...await original<object>(), watchImport: calls.watch }));
vi.mock('@/features/people/ImportProgress', () => ({ ImportProgress: ({ id, onReport }: { id: string; onReport: (id: string) => void }) => <div>Running {id}<button onClick={() => onReport('report-1')}>Finish job</button></div> }));
vi.mock('@/features/people/ImportReport', () => ({ ImportReport: ({ id }: { id: string }) => <div>Report {id}</div> }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/people/importUploads', () => ({ ImportUploads: class {
  constructor(readonly source: { files: { file: File; path: string }[]; bytes: number }) {}
  run(_signal: AbortSignal, progress: (value: { received: number; bytes: number; completed: number; files: number }) => void) {
    if (calls.uploadFails) { calls.uploadFails = false; return Promise.reject(new Error('Upload failed')); }
    progress({ received: this.source.bytes, bytes: this.source.bytes, completed: this.source.files.length, files: this.source.files.length });
    return Promise.resolve(this.source.files.map((entry) => ({ ...entry, blob: { blob: 'a'.repeat(64), name: entry.file.name, size: entry.file.size, media_type: entry.file.type } })));
  }
  cancel() { return Promise.resolve(); }
} }));

const user = userRef(me(1, 'ada', 'Ada Lovelace'));
const node = org('place', 'acme', 'Acme', user, { access: { ...memberAccess, administers: true, permissions: ['org.interact', 'doc.create'] } });
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: (id: string) => ({ queryKey: ['wizard-node', id], queryFn: () => Promise.resolve(node), retry: false }) }));
vi.mock('@/api/documentTree', async (original) => ({ ...await original<object>(),
  documentDirectoryQuery: (request: { node: string }) => ({ queryKey: ['wizard-root', request.node], queryFn: () => Promise.resolve({ directory: { node: { id: 'place' }, can_create: true } }), retry: false }),
  documentTreeEntryQuery: (item: { id: string }) => ({ queryKey: ['wizard-folder', item.id], queryFn: () => Promise.resolve({ directory: { node: { id: 'place' }, can_create: true } }), retry: false }),
}));

const plan: ImportPlan = {
  node: { id: 'place', kind: 'org', name: 'Acme', redacted: false }, source: { mode: 'package', entries: 1, bytes: 10, package: null },
  counts: { places: 0, folders: 0, documents: 1, chats: 0, threads: 0, files: 0, messages: 0, turns: 0, comments: 0, versions: 0 },
  actions: { create: 1, keep_both: 0, replace: 0, skip: 0 }, conflicts: 0, bytes: 10, limits: { max_bytes: 1024, max_items: 20 },
  authors: 'labels', access: { carried: false, allowed: true, restored: false }, people: [], losses: [], blockers: [],
  objects: { items: [{ source_id: 'source', kind: 'document', name: 'Plan', parent: null, path: [], bytes: 10, action: 'create', allowed: ['create', 'skip'], existing: null, blocked: null, creator: null, created_at: null }], next_cursor: null },
};
const job = { id: 'job-1', kind: 'import', state: 'running', result: null, progress: { percent: 1 } } as Job;

beforeEach(() => {
  calls.plan.mockReset(); calls.start.mockReset(); calls.watch.mockReset(); calls.uploadFails = false;
  calls.plan.mockResolvedValue(plan); calls.start.mockResolvedValue(job);
});

async function atDestination() {
  const user = userEvent.setup();
  show(<ImportDialog target={{ kind: 'new', place: { id: 'place', label: 'Acme' }, folder: null }} onClose={vi.fn()} />);
  fireEvent.change(screen.getByTestId('people-import-files'), { target: { files: [new File(['package'], 'backup.zip')] } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await screen.findByRole('radio', { name: 'New place inside' });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
  return user;
}

it('recognises a package from preflight, reviews options and follows its accepted job', async () => {
  const person = await atDestination();
  await person.click(screen.getByRole('radio', { name: 'Into this place' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
  await person.click(screen.getByRole('button', { name: 'Next' }));
  expect(await screen.findByText('Plan')).toBeVisible();
  await person.click(screen.getByRole('switch', { name: 'Comments' }));
  await person.click(screen.getByRole('checkbox', { name: 'Notify me when done' }));
  await waitFor(() => expect(screen.getByRole('button', { name: /^Import$/ })).toBeEnabled());
  await person.click(screen.getByRole('button', { name: /^Import$/ }));
  expect(await screen.findByText('Running job-1')).toBeVisible();
  const body = calls.start.mock.calls[0]?.[1] as ImportRequest;
  expect(body.source.files).toEqual([{ name: 'backup.zip', blob: 'a'.repeat(64) }]);
  expect(body.destination.root).toBe('here');
  expect(body.options.comments).toBe(false);
  expect(body.options.authors).toBe('labels');
  expect(calls.watch).toHaveBeenCalledWith(job, 'place', true);
  await person.click(screen.getByRole('button', { name: 'Finish job' }));
  expect(await screen.findByText('Report report-1')).toBeVisible();
});

it('keeps a blocked plan visible and does not start an import', async () => {
  calls.plan.mockResolvedValue({ ...plan, blockers: [{ code: 'import.too_many', message: 'Too many' }] });
  const person = await atDestination();
  await person.click(screen.getByRole('button', { name: 'Next' }));
  expect(await screen.findByText('This import would hold more things than the place allows. Choose less.')).toBeVisible();
  expect(screen.getByRole('button', { name: /^Import$/ })).toBeDisabled();
  expect(calls.start).not.toHaveBeenCalled();
});

function chooseReviewTab(name: string) {
  const tab = within(screen.getByRole('tablist', { name: 'Review' })).getByRole('tab', { name: new RegExp(`^${name}`) });
  fireEvent.keyDown(tab, { key: 'Enter' });
  expect(tab).toHaveAttribute('aria-selected', 'true');
}

it('reuses one review plan across every tab and fetches a new plan only after a choice changes', async () => {
  await atDestination();
  fireEvent.click(screen.getByRole('button', { name: 'Next' }));
  await screen.findByText('Plan');
  await waitFor(() => expect(screen.getByRole('button', { name: /^Import$/ })).toBeEnabled());
  expect(calls.plan).toHaveBeenCalledTimes(2); // Source inspection, then the reviewed choices.
  for (const tab of ['People', 'Conflicts', 'Not imported', 'Content', 'Conflicts', 'Content']) {
    chooseReviewTab(tab);
    expect(calls.plan).toHaveBeenCalledTimes(2);
  }
  fireEvent.click(screen.getByRole('switch', { name: 'Comments' }));
  await waitFor(() => expect(calls.plan).toHaveBeenCalledTimes(3));
  await waitFor(() => expect(screen.getByRole('button', { name: /^Import$/ })).toBeEnabled());
  expect((calls.plan.mock.calls[2]?.[1] as ImportRequest).options.comments).toBe(false);
  for (const tab of ['People', 'Conflicts', 'Content']) {
    chooseReviewTab(tab);
    expect(calls.plan).toHaveBeenCalledTimes(3);
  }
});

it('keeps the chosen source available after an upload failure', async () => {
  calls.uploadFails = true;
  const person = userEvent.setup();
  show(<ImportDialog target={{ kind: 'new', place: { id: 'place', label: 'Acme' }, folder: null }} onClose={vi.fn()} />);
  fireEvent.change(screen.getByTestId('people-import-files'), { target: { files: [new File(['text'], 'plan.xhtml'), new File(['doc'], 'report.docx')] } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
  await person.click(screen.getByRole('button', { name: 'Next' }));
  expect(await screen.findByRole('button', { name: 'Retry' })).toBeEnabled();
  expect(screen.getByText('plan.xhtml')).toBeVisible();
  expect(screen.getByText('report.docx')).toBeVisible();
  await person.click(screen.getByRole('button', { name: 'Retry' }));
  expect(await screen.findByRole('radio', { name: 'New place inside' })).toBeVisible();
});

it('does not hand a late accepted job to a different reader', async () => {
  const before = client.session.getState();
  client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
  try {
    let finish: (job: Job) => void = () => { throw new Error('Import was not started'); };
    calls.start.mockImplementation(() => new Promise<Job>((resolve) => { finish = resolve; }));
    const person = await atDestination();
    await person.click(screen.getByRole('button', { name: 'Next' }));
    await screen.findByText('Plan');
    await waitFor(() => expect(screen.getByRole('button', { name: /^Import$/ })).toBeEnabled());
    await person.click(screen.getByRole('button', { name: /^Import$/ }));
    expect(calls.start).toHaveBeenCalledOnce();
    act(() => { client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' }); });
    await act(async () => { finish(job); await Promise.resolve(); });
    expect(calls.watch).not.toHaveBeenCalled();
    expect(screen.queryByText('Running job-1')).not.toBeInTheDocument();
  } finally { client.session.setState(before); }
});
