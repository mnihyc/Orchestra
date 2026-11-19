/** The wizard carries selected files and reviewed choices into one resumable background import. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ImportDialog } from '@/features/people/ImportDialog';
import type { ImportReview as ImportReviewComponent } from '@/features/people/ImportReview';
import type { ImportPlan, ImportRequest, ImportView, Job } from '@/generated/api-types';
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

// Exercise the wizard's real query and page replacement without mounting fifty independent
// editor controls. The review's native choices, tab filtering and previews have their own tests.
vi.mock('@/features/people/ImportReview', async () => {
  const { useState } = await import('react');
  const { Tabs } = await import('@/ui/Tabs');
  const { ImportPagination } = await import('@/features/people/ImportPagination');
  return { ImportReview: function ReviewBoundary({ rows, hasMore, firstPage, loadingMore, moreError, onMore, onFirst }: Parameters<typeof ImportReviewComponent>[0]) {
    const [tab, setTab] = useState('content');
    const shown = rows.filter((row) => tab !== 'conflicts' || row.existing !== null || row.blocked !== null);
    const panel = <>
      {shown.length === 0 ? <div>More items may match</div>
        : <table aria-label={tab === 'conflicts' ? 'Conflicts' : 'Content'}><tbody>{shown.map((row) => <tr key={row.source_id}><td>{row.name}</td></tr>)}</tbody></table>}
      <ImportPagination first={firstPage === false} more={hasMore} loading={loadingMore} error={moreError} onMore={onMore} onFirst={onFirst ?? (() => undefined)} />
    </>;
    return <Tabs label="Review" value={tab} onValueChange={setTab} tabs={[{ value: 'content', label: 'Content', panel }, { value: 'conflicts', label: 'Conflicts', panel }]} />;
  } };
});

function chooseReviewTab(name: string) {
  const tab = within(screen.getByRole('tablist', { name: 'Review' })).getByRole('tab', { name: new RegExp(`^${name}`) });
  fireEvent.keyDown(tab, { key: 'Enter' });
  expect(tab).toHaveAttribute('aria-selected', 'true');
}

it('keeps a million-item plan to one page and asks again only when the person requests another page', async () => {
  const first = plan.objects.items[0];
  if (first === undefined) throw new Error('Fixture has no item');
  calls.plan.mockImplementation((_node: string, _request: ImportRequest, view: ImportView) => {
    expect(view.kind).toBeUndefined();
    expect(view.conflicts_only).toBeUndefined();
    const offset = Number(view.cursor ?? 0);
    return Promise.resolve({ ...plan, counts: { ...plan.counts, documents: 1_000_000 }, conflicts: 1,
      objects: { items: Array.from({ length: 50 }, (_, index) => ({ ...first,
        source_id: `source-${offset + index}`, name: `Document ${offset + index}`,
        existing: offset + index === 50 ? { id: 'existing', kind: 'document', name: 'Earlier document', found_by: 'name', updated_at: null } : null,
      })), next_cursor: String(offset + 50) },
    });
  });
  await atDestination();
  fireEvent.click(screen.getByRole('button', { name: 'Next' }));
  await screen.findByText('Document 0');
  const importButton = screen.getByText('Import', { selector: 'button' });
  await waitFor(() => expect(importButton).toBeEnabled());
  expect(calls.plan).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('table', { name: 'Content' }).querySelectorAll('tbody tr').length).toBeLessThanOrEqual(50);
  chooseReviewTab('Conflicts');
  expect(screen.getByText('More items may match')).toBeVisible();
  expect(calls.plan).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByText('Next page', { selector: 'button' }));
  expect(await screen.findByText('Document 50')).toBeVisible();
  expect(screen.queryByText('Document 0')).not.toBeInTheDocument();
  expect(calls.plan).toHaveBeenCalledTimes(3);
  chooseReviewTab('Content');
  expect(screen.getByText('Document 50')).toBeVisible();
  expect(screen.queryByText('Document 0')).not.toBeInTheDocument();
  expect(screen.getByRole('table', { name: 'Content' }).querySelectorAll('tbody tr').length).toBeLessThanOrEqual(50);
  expect(calls.plan).toHaveBeenCalledTimes(3);
  fireEvent.click(screen.getByText('Back to first page', { selector: 'button' }));
  expect(await screen.findByText('Document 0')).toBeVisible();
  expect(screen.queryByText('Document 50')).not.toBeInTheDocument();
  expect(calls.plan).toHaveBeenCalledTimes(4);
});

