/** Import destinations follow the place and folder's actual current creation standing. */
import { fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { ImportDestination, type ImportDestinationValue, useImportDestinationStanding } from '@/features/people/ImportDestination';
import type { DocumentDirectory, ImportSourceMode } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ node: vi.fn(), root: vi.fn(), folder: vi.fn() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: (id: string) => ({ queryKey: ['import-dest-node', id], queryFn: () => calls.node(id) as Promise<unknown>, retry: false }) }));
vi.mock('@/api/documentTree', async (original) => ({ ...await original<object>(),
  documentDirectoryQuery: (request: { node: string; at: DocumentDirectory; cursor?: string | null }) => ({ queryKey: ['import-dest-root', request.node, request.at.kind, request.at.id, request.cursor ?? null], queryFn: () => calls.root(request.node, request.at, request.cursor ?? null) as Promise<unknown>, retry: false }),
  documentTreeEntryQuery: (item: { id: string }) => ({ queryKey: ['import-dest-folder', item.id], queryFn: () => calls.folder(item.id) as Promise<unknown>, retry: false }),
}));

const destination: ImportDestinationValue = { place: { id: 'place', label: 'Acme / Web' }, folder: null, root: 'inside' };
function Probe({ mode, initial = destination, packageSource = false }: { mode: ImportSourceMode | null; initial?: ImportDestinationValue; packageSource?: boolean }) {
  const [value, setValue] = useState(initial);
  const standing = useImportDestinationStanding(value, mode);
  return <><ImportDestination value={value} onChange={setValue} standing={standing} isPackage={packageSource} /><button disabled={!standing.allowed}>Next</button></>;
}

beforeEach(() => {
  calls.node.mockReset(); calls.root.mockReset(); calls.folder.mockReset();
  calls.node.mockResolvedValue({ id: 'place', kind: 'project', org_id: 'org', archived: false, path: [{ id: 'place', kind: 'project', name: 'Web', redacted: false }], access: { permissions: ['project.interact'] } });
  calls.root.mockResolvedValue({ directory: { node: { id: 'place' }, can_create: true } });
});

it('does not import plain files where document creation is disabled', async () => {
  calls.root.mockResolvedValue({ directory: { node: { id: 'place' }, can_create: false } });
  show(<Probe mode="files" />);
  await waitFor(() => expect(screen.getByText('You cannot create documents in this folder')).toBeVisible());
  expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
});

it('allows a package to be planned where the person can create a place even if documents are disabled', async () => {
  calls.root.mockResolvedValue({ directory: { node: { id: 'place' }, can_create: false } });
  show(<Probe mode="package" packageSource />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
  const here = screen.getByRole('radio', { name: 'Into this place' });
  fireEvent.click(here);
  expect(here).toBeChecked();
});

it('refuses a selected document folder from a different place', async () => {
  calls.folder.mockResolvedValue({ directory: { node: { id: 'elsewhere' }, can_create: true } });
  show(<Probe mode="files" initial={{ ...destination, folder: { id: 'folder', label: 'Other folder' } }} />);
  await waitFor(() => expect(screen.getByText('Choose a folder in the selected place')).toBeVisible());
  expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
});

it('keeps an archived destination unavailable even with creation rights', async () => {
  calls.node.mockResolvedValue({ id: 'place', kind: 'project', org_id: 'org', archived: true, path: [{ id: 'place', kind: 'project', name: 'Web', redacted: false }], access: { permissions: ['project.interact', 'doc.create'] } });
  show(<Probe mode="package" />);
  await waitFor(() => expect(screen.getByText('This place is archived')).toBeVisible());
  expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
});

it('does not keep a stale destination name when its current read fails', async () => {
  calls.node.mockRejectedValue(new Error('Unavailable'));
  show(<Probe mode="files" initial={{ ...destination, place: { id: 'place', label: 'Private place' } }} />);
  await screen.findByRole('alert');
  expect(screen.queryByDisplayValue('Private place')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
});

it('uses a folder selected by browsing the chosen place', async () => {
  const child = { entry: { item: { kind: 'container', id: 'notes' }, title: 'Notes', basename: 'Notes', deleted_at: null } };
  calls.root.mockImplementation((_node: string, at: DocumentDirectory) => Promise.resolve({
    directory: { node: { id: 'place' }, at, folder: at.kind === 'container' ? child : null, can_create: true },
    items: at.kind === 'node_docs' ? [child] : [], next_cursor: null,
  }));
  calls.folder.mockResolvedValue({ directory: { node: { id: 'place' }, can_create: true } });
  const user = userEvent.setup();
  show(<Probe mode="files" />);
  await user.click(screen.getByRole('button', { name: 'Browse folders' }));
  await user.click(await screen.findByRole('button', { name: 'Notes' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Use this folder' })).not.toHaveAttribute('aria-disabled'));
  await user.click(screen.getByRole('button', { name: 'Use this folder' }));
  expect(await screen.findByDisplayValue('Notes')).toBeVisible();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
});
