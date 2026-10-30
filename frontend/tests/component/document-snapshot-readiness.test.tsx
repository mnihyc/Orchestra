/** A real readonly Tiptap snapshot reports its existing editor without starting live synchronization. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Sync from '@/features/documents/sync';
import { BlockEditor, type EditorApi } from '@/features/documents/Editor';
import { DocumentMarksProvider, retireDocumentMarks } from '@/features/documents/marks';
import { World } from '../doubles/world';
import { show } from './support';

const sync = vi.hoisted(() => ({ create: vi.fn<typeof Sync.createSync>() }));
vi.mock('@/features/documents/sync', async (load) => ({ ...await load<typeof Sync>(), createSync: sync.create }));
afterEach(() => { cleanup(); retireDocumentMarks(); sync.create.mockReset(); });

it('calls onReady for the snapshot with sync null and retires readiness on unmount', async () => {
  const world = new World(), ada = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((value) => value.title === 'Rollout plan');
  if (ada === undefined || stored === undefined) throw new Error('The ordinary readable document fixture is required.');
  const doc = world.documents.full(ada, stored), heading = world.documents.sync(ada, stored, null).blocks.find((block) => block.type === 'heading');
  if (heading === undefined) throw new Error('The native heading fixture is required.');
  const ready = vi.fn<(api: EditorApi | null) => void>();
  // Inspection disables unrelated suggestion/collaboration surfaces; fixed is the production snapshot branch.
  const drawn = show(<DocumentMarksProvider owner="snapshot-pane" documentId={doc.id}>
    <BlockEditor document={doc} editable={false} inspection fixed={[heading]} onReady={ready} />
  </DocumentMarksProvider>);
  await screen.findByText('Rollout plan');
  await waitFor(() => expect(ready).toHaveBeenCalledWith(expect.objectContaining({ sync: null })));
  const editor = ready.mock.calls.find(([api]) => api !== null)?.[0]?.editor;
  expect(editor?.isEditable).toBe(false); expect(sync.create).not.toHaveBeenCalled();
  drawn.unmount(); expect(ready.mock.calls.at(-1)?.[0]).toBeNull(); expect(sync.create).not.toHaveBeenCalled();
});
