/** A private import draft cannot remain visible when the signed-in reader changes. */
import { act, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { ImportHost } from '@/features/people/ImportHost';
import { closeImport, openImport, useImportHost } from '@/features/people/importHostState';
import { show } from './support';

vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => vi.fn() }));
vi.mock('@/features/people/importJobs', async (original) => ({ ...await original<object>(), useImportJobWatch: () => undefined }));
vi.mock('@/features/people/ImportDialog', () => ({ ImportDialog: () => <div>Private import draft</div> }));

const before = client.session.getState();
afterEach(() => { closeImport(); client.session.setState(before); });

it('closes the previous reader’s draft when another session signs in', async () => {
  client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
  openImport({ id: 'place', label: 'Private place' });
  show(<ImportHost />);
  expect(await screen.findByText('Private import draft')).toBeVisible();
  act(() => client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' }));
  expect(screen.queryByText('Private import draft')).not.toBeInTheDocument();
  await waitFor(() => expect(useImportHost.getState().target).toBeNull());
});

it('clears a draft when the same person receives a different authority session', async () => {
  client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
  openImport({ id: 'place', label: 'Private place' });
  show(<ImportHost />);
  expect(await screen.findByText('Private import draft')).toBeVisible();
  act(() => client.session.setState({ userId: 'first', readerVersion: 2, status: 'signed_in' }));
  expect(screen.queryByText('Private import draft')).not.toBeInTheDocument();
  await waitFor(() => expect(useImportHost.getState().target).toBeNull());
});
