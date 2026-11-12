/** A tree-supplied parent caption needs no independent live reader for every menu button. */
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import { NodeMenuButton } from '@/features/navigation/NodeMenu';
import type { PlaceRef } from '@/features/navigation/places';
import { treeWriter, useTreeNode } from '@/realtime/stores/tree';
import type * as TreeModule from '@/realtime/stores/tree';
import { TooltipProvider } from '@/ui/Tooltip';
import { treeNode } from '../doubles/tree-node';

vi.mock('@/realtime/stores/tree', async (original) => {
  const actual = await original<typeof TreeModule>();
  return { ...actual, useTreeNode: vi.fn(actual.useTreeNode) };
});

afterEach(() => { cleanup(); treeWriter.clear(); vi.clearAllMocks(); });

const parent = treeNode({ id: 'parent', kind: 'project', parent_id: 'org', name: 'Parent', tree_seq: 1 });
const place: PlaceRef = { id: 'child', name: 'Child', kind: 'topic', parentId: parent.id, status: 'open', archived: false, folder: false };

it('uses the supplied parent caption without subscribing to native tree rows', () => {
  treeWriter.reset('org', [parent], 1);
  const view = render(<NodeMenuButton place={place} parentName="Parent" />, { wrapper: TooltipProvider });
  expect(screen.getByRole('button', { name: /Child.*Parent/ })).toBeVisible();
  expect(useTreeNode).not.toHaveBeenCalled();
  view.rerender(<NodeMenuButton place={place} parentName="Renamed" />);
  expect(screen.getByRole('button', { name: /Child.*Renamed/ })).toBeVisible();
  expect(useTreeNode).not.toHaveBeenCalled();
});

it('respects an explicitly unavailable parent caption', () => {
  treeWriter.reset('org', [parent], 1);
  render(<NodeMenuButton place={place} parentName={null} />, { wrapper: TooltipProvider });
  expect(screen.getByRole('button', { name: /Child/ })).not.toHaveAccessibleName(/Parent/);
  expect(useTreeNode).not.toHaveBeenCalled();
});

it('keeps the independent parent reader when no caption was supplied', () => {
  treeWriter.reset('org', [parent], 1);
  render(<NodeMenuButton place={place} />, { wrapper: TooltipProvider });
  expect(screen.getByRole('button', { name: /Child.*Parent/ })).toBeVisible();
  act(() => treeWriter.apply('org', [{ ...parent, name: 'Renamed', tree_seq: 2 }], 2));
  expect(screen.getByRole('button', { name: /Child.*Renamed/ })).toBeVisible();
  act(() => treeWriter.apply('org', [{ ...parent, deleted: true, tree_seq: 3 }], 3));
  expect(screen.getByRole('button', { name: /Child/ })).not.toHaveAccessibleName(/Parent|Renamed/);
});
