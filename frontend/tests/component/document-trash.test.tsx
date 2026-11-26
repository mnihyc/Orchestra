/** A document or folder Delete is no dialog: the rows go to the trash in one tree act, without a preview, and the line
 * after it undoes exactly that act. The sidebar's Delete of a document does the same. */
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { organizeDocuments } from '@/api/documentTree';
import { TreeActionDialog } from '@/features/documents/TreeActions';
import { NodeDialogHost } from '@/features/navigation/NodeDialogHost';
import { openNodeDialog, useDialogState } from '@/features/navigation/NodeDialogs';
import type { DocumentTreeListed, DocumentTreeOperationBody, DocumentTreeReceipt } from '@/generated/api-types';
import { show } from './support';

vi.mock('@/api/documentTree', async (original) => ({ ...await original<object>(), organizeDocuments: vi.fn() }));

const receipt = { operation: 'op', entries: [], copied: [], gaining: 0, losing: 0, restore_until: null } as unknown as DocumentTreeReceipt;
const row = (id: string, kind: 'document' | 'container' = 'document'): DocumentTreeListed => ({
  entry: { item: { kind, id }, title: `Title ${id}`, basename: `title-${id}`, node: 'topic-1', root: 'org-1', parent: { kind: 'node_docs', id: 'topic-1' },
    deleted_at: null, deletion: null, files: 0, versions: 1, owner: 'me', revision: 1 },
  standing: { manage: true, read: true, write: true }, document: null, inspection: null,
} as unknown as DocumentTreeListed);
const sent = (index: number): DocumentTreeOperationBody => vi.mocked(organizeDocuments).mock.calls[index]?.[0] as DocumentTreeOperationBody;

beforeEach(() => { vi.clearAllMocks(); vi.mocked(organizeDocuments).mockResolvedValue(receipt); });
afterEach(() => { cleanup(); useDialogState.setState({ current: null, open: false }); });

it('deletes the chosen rows in one act with no preview, and Undo restores the same entries', async () => {
  const onClose = vi.fn();
  const onDone = vi.fn();
  show(<TreeActionDialog command="delete" rows={[row('a'), row('b', 'container')]} onClose={onClose} onDone={onDone} />);
  await vi.waitFor(() => expect(organizeDocuments).toHaveBeenCalledTimes(1));
  expect(sent(0)).toMatchObject({ command: 'delete', items: [{ kind: 'document', id: 'a' }, { kind: 'container', id: 'b' }], deletion_plan: null, to: null });
  expect(onClose).toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await vi.waitFor(() => expect(onDone).toHaveBeenCalled());
  expect(await screen.findByText('Moved 2 items to Trash')).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Undo' }));
  await vi.waitFor(() => expect(organizeDocuments).toHaveBeenCalledTimes(2));
  expect(sent(1)).toMatchObject({ command: 'restore', items: [{ kind: 'document', id: 'a' }, { kind: 'container', id: 'b' }] });
  expect(sent(1).client_id).not.toBe(sent(0).client_id);
  expect(await screen.findByText('2 items are back')).toBeInTheDocument();
});

it('the sidebar Delete of a document moves it to the trash at once', async () => {
  show(<NodeDialogHost />);
  act(() => openNodeDialog({ type: 'delete-document', doc: { id: 'doc-1', title: 'Launch notes', node: { id: 'topic-1', kind: 'topic' }, archived: false, owner: null } }));
  await vi.waitFor(() => expect(organizeDocuments).toHaveBeenCalledTimes(1));
  expect(sent(0)).toMatchObject({ command: 'delete', items: [{ kind: 'document', id: 'doc-1' }], deletion_plan: null });
  expect(await screen.findByText('Moved “Launch notes” to Trash')).toBeInTheDocument();
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
});
