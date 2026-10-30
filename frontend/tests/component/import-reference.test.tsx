/** A report checks a document folder's current access only when its reference is opened. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { MouseEventHandler, ReactNode } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type * as References from '@/api/references';
import { ImportReference } from '@/features/people/ImportReference';
import type { TypedReference } from '@/features/people/references';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn(), preview: vi.fn() }));
vi.mock('@/api/documentTree', async (original) => ({ ...await original<object>(), documentTreeEntryQuery: (item: { id: string }, _cache: unknown, _retained: boolean, scope: string) => ({ queryKey: ['folder-reference', item.id, scope], queryFn: () => calls.read(item.id) as Promise<unknown>, retry: false }) }));
vi.mock('@/api/references', async (original) => {
  const actual = await original<typeof References>();
  return { ...actual, referencePreviewQuery: (reference: TypedReference, scope?: string) => ({ ...actual.referencePreviewQuery(reference, scope), queryFn: () => calls.preview(reference) as Promise<unknown> }) };
});
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), Link: ({ children, onClick }: { children: ReactNode; onClick?: MouseEventHandler<HTMLAnchorElement> }) => <a href="/document-folder/folder" onClick={(event) => { onClick?.(event); event.preventDefault(); }}>{children}</a> }));
beforeEach(() => { calls.read.mockReset(); calls.preview.mockReset(); });

it('loads a folder only on demand and offers Open after its standing is checked', async () => {
  calls.read.mockResolvedValue({ directory: { folder: { standing: { read: true } } } });
  const user = userEvent.setup();
  const onOpen = vi.fn();
  show(<ImportReference id="folder" kind="folder" name="Notes" onOpen={onOpen} />);
  expect(calls.read).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Notes' }));
  const link = await screen.findByRole('link', { name: 'Open folder' });
  expect(link).toBeVisible();
  await user.click(link);
  expect(onOpen).toHaveBeenCalledOnce();
  expect(calls.read).toHaveBeenCalledExactlyOnceWith('folder');
});

it('does not offer a folder link when the current read is refused', async () => {
  calls.read.mockRejectedValue(new Error('Denied'));
  const user = userEvent.setup();
  show(<ImportReference id="folder" kind="folder" name="Notes" />);
  await user.click(screen.getByRole('button', { name: 'Notes' }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByRole('link', { name: 'Open folder' })).not.toBeInTheDocument();
});

it('does not reuse a folder’s standing after the reader changes', async () => {
  const before = client.session.getState();
  client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
  try {
    calls.read.mockResolvedValueOnce({ directory: { folder: { standing: { read: true } } } }).mockImplementationOnce(() => new Promise(() => undefined));
    const user = userEvent.setup();
    show(<ImportReference id="folder" kind="folder" name="Notes" />);
    await user.click(screen.getByRole('button', { name: 'Notes' }));
    expect(await screen.findByRole('link', { name: 'Open folder' })).toBeVisible();
    act(() => { client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' }); });
    expect(screen.queryByRole('link', { name: 'Open folder' })).not.toBeInTheDocument();
    await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
  } finally { client.session.setState(before); }
});

it('does not reuse a reference preview after the reader changes', async () => {
  const before = client.session.getState();
  client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
  try {
    calls.preview.mockResolvedValueOnce({ kind: 'document', id: 'doc', node_kind: null, title: 'Private title', place: null,
      description: 'Private description', updated_at: null, updated_by: null, part: null }).mockImplementationOnce(() => new Promise(() => undefined));
    const user = userEvent.setup();
    show(<ImportReference id="doc" kind="document" name="Shared label" />);
    await user.click(screen.getByRole('link', { name: 'Shared label' }));
    expect(await screen.findByText('Private description')).toBeVisible();
    act(() => { client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' }); });
    expect(screen.queryByText('Private description')).not.toBeInTheDocument();
    await waitFor(() => expect(calls.preview).toHaveBeenCalledTimes(2));
  } finally { client.session.setState(before); }
});

it('checks folder access again before reopening a retained link', async () => {
  calls.read.mockResolvedValueOnce({ directory: { folder: { standing: { read: true } } } }).mockImplementationOnce(() => new Promise(() => undefined));
  const user = userEvent.setup();
  show(<ImportReference id="folder" kind="folder" name="Notes" />);
  await user.click(screen.getByRole('button', { name: 'Notes' }));
  expect(await screen.findByRole('link', { name: 'Open folder' })).toBeVisible();
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('button', { name: 'Notes' }));
  expect(screen.queryByRole('link', { name: 'Open folder' })).not.toBeInTheDocument();
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
});

it('checks reference access again before reopening its card', async () => {
  calls.preview.mockResolvedValueOnce({ kind: 'document', id: 'doc', node_kind: null, title: 'Private title', place: null,
    description: 'Private description', updated_at: null, updated_by: null, part: null }).mockImplementationOnce(() => new Promise(() => undefined));
  const user = userEvent.setup();
  show(<ImportReference id="doc" kind="document" name="Shared label" />);
  await user.click(screen.getByRole('link', { name: 'Shared label' }));
  expect(await screen.findByText('Private description')).toBeVisible();
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('link', { name: 'Shared label' }));
  expect(screen.queryByText('Private description')).not.toBeInTheDocument();
  await waitFor(() => expect(calls.preview).toHaveBeenCalledTimes(2));
});
