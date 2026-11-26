/** A selection's Delete is one act: no review, each topic to the trash in turn, one line for the set, one Undo for the set. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { deleteNode, previewDelete, restoreNode } from '@/api/nodes';
import { TopicSelection } from '@/features/project/TopicSelection';
import type { Node, Topic } from '@/generated/api-types';
import { show } from './support';

vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), deleteNode: vi.fn(), restoreNode: vi.fn(), previewDelete: vi.fn() }));

const topic = (id: string): Topic => ({ id, kind: 'topic', name: `Topic ${id}`, parent_id: 'project-1', org_id: 'org-1', status: 'open', revision: 1, archived: false, path: [], access: { permissions: ['topic.manage', 'topic.interact'] } } as unknown as Topic);

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(deleteNode).mockImplementation((id) => id === 'b' ? Promise.reject(new Error('refused')) : Promise.resolve({ deletion_id: `group-${id}` }));
  vi.mocked(restoreNode).mockResolvedValue({ id: 'x' } as unknown as Node);
});
afterEach(cleanup);

it('moves the selection to the trash with no review, names the one refused, and undoes the rest together', async () => {
  const onClear = vi.fn();
  show(<TopicSelection selected={[topic('a'), topic('b'), topic('c')]} onClear={onClear} />);
  await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
  await vi.waitFor(() => expect(deleteNode).toHaveBeenCalledTimes(3));
  expect(previewDelete).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(await screen.findByText('Moved 2 of 3 to Trash')).toBeInTheDocument();
  expect(screen.getByText(/Not moved: Topic b/)).toBeInTheDocument();
  expect(onClear).toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Undo' }));
  await vi.waitFor(() => expect(restoreNode).toHaveBeenCalledTimes(2));
  expect(vi.mocked(restoreNode).mock.calls).toEqual([['c', 'group-c'], ['a', 'group-a']]);
  expect(await screen.findByText('2 items are back')).toBeInTheDocument();
});
