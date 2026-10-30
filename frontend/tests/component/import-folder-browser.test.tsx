/** A destination stays reachable even when global name search has a shorter result limit. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ImportFolderBrowser } from '@/features/people/ImportFolderBrowser';
import type { DocumentDirectory } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn() }));
vi.mock('@/api/documentTree', async (original) => ({ ...await original<object>(),
  documentDirectoryQuery: (request: { node: string; at: DocumentDirectory; cursor: string | null }) => ({
    queryKey: ['folder-browser', request.node, request.at.kind, request.at.id, request.cursor], queryFn: () => calls.read(request) as Promise<unknown>, retry: false,
  }),
}));

const root: DocumentDirectory = { kind: 'node_docs', id: 'place' };
const folder: DocumentDirectory = { kind: 'container', id: 'folder' };
const child = { entry: { item: { kind: 'container', id: 'folder' }, title: 'Notes', basename: 'Notes', deleted_at: null } };
const document = { entry: { item: { kind: 'document', id: 'document' }, title: 'Page', basename: 'Page', deleted_at: null } };
const response = (at: DocumentDirectory, items: unknown[], next_cursor: string | null, can_create = true) => ({
  directory: { node: { id: 'place' }, at, folder: at.kind === 'container' ? child : null, can_create }, items, next_cursor,
});

beforeEach(() => { calls.read.mockReset(); });

it('pages through a place and selects a nested folder', async () => {
  calls.read.mockImplementation(({ at, cursor }: { at: DocumentDirectory; cursor: string | null }) => Promise.resolve(at.kind === 'container'
    ? response(folder, [], null) : cursor === null ? response(root, [document], 'next') : response(root, [child], null)));
  const onChoose = vi.fn(), user = userEvent.setup();
  show(<ImportFolderBrowser place="place" onChoose={onChoose} />);
  expect(calls.read).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Browse folders' }));
  expect(await screen.findByText('No folders on this page')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Next page' }));
  await user.click(await screen.findByRole('button', { name: 'Notes' }));
  expect(await screen.findByText('Documents / Notes')).toHaveFocus();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Use this folder' })).not.toHaveAttribute('aria-disabled'));
  await user.click(screen.getByRole('button', { name: 'Parent folder' }));
  expect(await screen.findByRole('button', { name: 'Notes' })).toBeVisible();
  const previous = screen.getByRole('button', { name: 'Previous page' });
  await waitFor(() => expect(previous).toBeEnabled());
  await user.click(previous);
  expect(await screen.findByText('No folders on this page')).toBeVisible();
  const next = screen.getByRole('button', { name: 'Next page' });
  await waitFor(() => expect(next).toBeEnabled());
  await user.click(next);
  const notes = await screen.findByRole('button', { name: 'Notes' });
  await waitFor(() => expect(notes).toBeEnabled());
  await user.click(notes);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Use this folder' })).not.toHaveAttribute('aria-disabled'));
  await user.click(screen.getByRole('button', { name: 'Use this folder' }));
  expect(onChoose).toHaveBeenCalledExactlyOnceWith({ id: 'folder', label: 'Notes' });
  expect(calls.read).toHaveBeenCalledWith({ node: 'place', at: root, cursor: 'next' });
  expect(calls.read).toHaveBeenCalledWith({ node: 'place', at: folder, cursor: null });
});

it('keeps a denied directory visible and allows a readable child to be checked', async () => {
  calls.read.mockImplementation(({ at }: { at: DocumentDirectory }) => Promise.resolve(at.kind === 'container' ? response(folder, [], null) : response(root, [child], null, false)));
  const onChoose = vi.fn(), user = userEvent.setup();
  show(<ImportFolderBrowser place="place" onChoose={onChoose} />);
  await user.click(screen.getByRole('button', { name: 'Browse folders' }));
  const blocked = await screen.findByRole('button', { name: 'Use this folder' });
  expect(blocked).toHaveAttribute('aria-disabled', 'true');
  await user.hover(blocked);
  expect(await screen.findByRole('tooltip')).toHaveTextContent('You cannot create documents in this folder');
  await user.click(blocked);
  expect(onChoose).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Notes' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Use this folder' })).not.toHaveAttribute('aria-disabled'));
  await user.click(screen.getByRole('button', { name: 'Use this folder' }));
  expect(onChoose).toHaveBeenCalledExactlyOnceWith({ id: 'folder', label: 'Notes' });
});

it('retries a directory read after it fails', async () => {
  calls.read.mockRejectedValueOnce(new Error('Offline')).mockResolvedValueOnce(response(root, [], null));
  const user = userEvent.setup();
  show(<ImportFolderBrowser place="place" onChoose={vi.fn()} />);
  await user.click(screen.getByRole('button', { name: 'Browse folders' }));
  await user.click(await screen.findByRole('button', { name: 'Try again' }));
  expect(await screen.findByText('No folders on this page')).toBeVisible();
  expect(calls.read).toHaveBeenCalledTimes(2);
});
