/** A document moves beside another place of its organization at once: nothing about access is read or said, the notice offers Undo. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import { MoveDialog } from '@/features/documents/Placement';
import type { Document } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { treeNode } from '../doubles/tree-node';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ preview: vi.fn(), move: vi.fn(), navigate: vi.fn() }));
vi.mock('@/api/documents', async (original) => ({ ...await original<typeof Documents>(), previewMove: calls.preview, moveDocument: calls.move }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => calls.navigate }));

const savedTrees = useTreeStore.getState();
let doc: Document;
beforeEach(() => {
  const world = new World(), ada = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((one) => one.title === 'Rollout plan');
  if (ada === undefined || stored === undefined) throw new Error('A readable document is required.');
  doc = world.documents.full(ada, stored);
  const root = doc.path[0]?.id;
  if (root === undefined) throw new Error('The document must have a visible root.');
  treeWriter.reset(root, [
    treeNode({ id: root, kind: 'org', parent_id: null, name: 'Acme' }),
    treeNode({ id: doc.node.id, kind: 'topic', parent_id: root, name: doc.node.name ?? 'Here' }),
    treeNode({ id: 'beside', kind: 'project', parent_id: root, name: 'Beside' }),
  ], 1);
  calls.move.mockImplementation((_id: string, nodeId: string) => Promise.resolve({ ...doc, node: { ...doc.node, id: nodeId, name: nodeId === 'beside' ? 'Beside' : doc.node.name } }));
  calls.navigate.mockResolvedValue(undefined);
});
afterEach(async () => { cleanup(); vi.resetAllMocks(); useTreeStore.setState(savedTrees); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`moves inside one organization without reading who gains or loses it, and Undo moves it back (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const close = vi.fn();
    show(<MoveDialog doc={doc} open onOpenChange={close} />);
    await user.click(screen.getByRole('option', { name: /Beside/ }));
    expect(screen.queryByText(i18n.t('documents.placement.nobody'))).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('documents.move.confirm') }));
    await waitFor(() => expect(calls.move).toHaveBeenCalledExactlyOnceWith(doc.id, 'beside'));
    expect(calls.preview).not.toHaveBeenCalled();
    expect(close).toHaveBeenCalledWith(false);
    await user.click(await screen.findByRole('button', { name: i18n.t('places.toast.undo') }));
    await waitFor(() => expect(calls.move).toHaveBeenLastCalledWith(doc.id, doc.node.id));
  });
}
