/** Existing document actions stay named when native current permissions or state make them unavailable. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as People from '@/api/people';
import type * as Nodes from '@/api/nodes';
import type * as Documents from '@/api/documents';
import type * as Settings from '@/api/settings';
import { PlaceReadme, useReadmeEntry } from '@/features/documents/Readme';
import { PutBack } from '@/features/people/PutBack';
import type { Document, DocumentBlock, DocumentChanges, Node } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Menu } from '@/ui/Menu';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ sync: vi.fn<typeof Documents.fetchDocumentSync>(), putBack: vi.fn<typeof People.putBackBlocks>(), update: vi.fn<typeof Nodes.updateNode>() }));
vi.mock('@/api/people', async (original) => ({ ...await original<typeof People>(), putBackBlocks: calls.putBack }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), updateNode: calls.update }));
vi.mock('@/api/documents', async (original) => ({ ...await original<typeof Documents>(), fetchDocumentSync: calls.sync }));
vi.mock('@/api/settings', async (original) => ({ ...await original<typeof Settings>(), settingsQuery: (id: string) => ({ queryKey: ['readme-preview-settings', id], queryFn: () => Promise.resolve([{ key: 'documents.readme_blocks', value: 1 }]) }) }));
vi.mock('@/features/documents/Editor', () => ({ BlockEditor: ({ fixed }: { fixed?: readonly DocumentBlock[] }) => <div>{fixed?.map((block) => <p key={block.id}>{block.id}</p>)}</div> }));
afterEach(async () => { cleanup(); vi.clearAllMocks(); await setLocale('en'); });
const after = { id: 'block', type: 'paragraph', content: [] } as unknown as DocumentBlock;
const change: DocumentChanges['blocks'][number] = { id: after.id, before: null, after, by: null };
const changes: DocumentChanges = { from: 0, to: 1, blocks: [change], authors: [], title: null };
function ReadmeAction({ place }: { place: Node | undefined }) {
  const entry = useReadmeEntry({ id: 'document' }, place);
  return <Menu label="Document actions" trigger={<button type="button">Document actions</button>} entries={entry === null ? [] : [entry]} />;
}

it.each(['en', 'zh-CN'] as const)('labels a README preview and keeps its full-document link even when the preview is complete (%s)', async (locale) => {
  await setLocale(locale);
  const world = new World(), ada = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((one) => one.title === 'Rollout plan');
  if (ada === undefined || stored === undefined) throw new Error('A readable README document is required.');
  const sync = world.documents.sync(ada, stored, null), first = sync.blocks[0];
  if (first === undefined) throw new Error('The README needs a block.');
  const blocks = [first, { ...first, id: 'second-preview-block', position: '2' }];
  calls.sync.mockResolvedValueOnce({ ...sync, blocks });
  const view = show(<PlaceReadme readme={sync.document} nodeId={sync.document.node.id} />);
  await screen.findByText(first.id);
  expect(screen.queryByText('second-preview-block')).not.toBeInTheDocument();
  expect(screen.getByText(i18n.t('documents.readme.preview'))).toBeVisible();
  expect(screen.getByRole('link', { name: i18n.t('documents.readme.open') })).toHaveAttribute('href', `/doc/${sync.document.id}`);
  view.unmount(); calls.sync.mockResolvedValueOnce({ ...sync, blocks: [first] });
  show(<PlaceReadme readme={sync.document} nodeId={sync.document.node.id} />);
  await screen.findByText(first.id);
  expect(screen.queryByText(i18n.t('documents.readme.preview'))).not.toBeInTheDocument();
  expect(screen.getByRole('link', { name: i18n.t('documents.readme.open') })).toHaveAttribute('href', `/doc/${sync.document.id}`);
});

for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['permission', 'archived', 'deleted'] as const)(`keeps Put back with its %s reason and performs no write (${locale})`, async (state) => {
    await setLocale(locale);
    const doc = { id: 'document', archived: state === 'archived', deleted: state === 'deleted', access: { permissions: state === 'permission' ? ['doc.view'] : ['doc.view', 'doc.edit'] } } as unknown as Document;
    const user = userEvent.setup();
    show(<PutBack doc={doc} change={change} changes={changes} onStale={vi.fn()} />);
    const act = screen.getByRole('button', { name: i18n.t('people.putBack') });
    expect(act).toHaveAttribute('aria-disabled', 'true');
    expect(act).toHaveAttribute('aria-description', i18n.t(state === 'permission' ? 'implicitAvailability.docEdit' : state === 'archived' ? 'implicitAvailability.docArchived' : 'implicitAvailability.docDeleted'));
    await user.click(act);
    expect(calls.putBack).not.toHaveBeenCalled();
  });
  it.each([false, true])(`keeps README setup in the document menu when place metadata or Manage is absent (known=%s, ${locale})`, async (known) => {
    await setLocale(locale);
    const user = userEvent.setup();
    const place = known ? { id: 'place', kind: 'topic', name: 'Visible place', revision: 1, readme: null, access: { permissions: ['node.view'] } } as unknown as Node : undefined;
    show(<ReadmeAction place={place} />);
    await user.click(screen.getByRole('button', { name: 'Document actions' }));
    const act = screen.getByRole('menuitem', { name: i18n.t('documents.readme.use') });
    expect(act).toHaveAttribute('aria-disabled', 'true');
    expect(act).toHaveAccessibleDescription(i18n.t(known ? 'implicitAvailability.placeManage' : 'implicitAvailability.placeUnavailable'));
    expect(calls.update).not.toHaveBeenCalled();
  });
  it(`keeps the existing eligible README write and its revision guard (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    const place = { id: 'place', kind: 'topic', name: 'Visible place', revision: 7, readme: null, access: { permissions: ['node.view', 'topic.manage'] } } as unknown as Node;
    calls.update.mockResolvedValue(place);
    show(<ReadmeAction place={place} />);
    await user.click(screen.getByRole('button', { name: 'Document actions' }));
    await user.click(screen.getByRole('menuitem', { name: i18n.t('documents.readme.use') }));
    await waitFor(() => expect(calls.update).toHaveBeenCalledExactlyOnceWith('place', { readme_document_id: 'document', expected_revision: 7 }));
  });
}
