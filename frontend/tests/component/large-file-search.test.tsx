/** Live filename search preserves its act and continuation without a complete file index. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import { NO_SEARCH, SearchBox, SearchResults } from '@/features/files/SearchInFiles';
import { show } from './support';

const calls = vi.hoisted(() => ({ names: vi.fn<typeof Workspace.searchNames>(), content: vi.fn<typeof Workspace.searchContent>() }));
vi.mock('@/api/workspace', async (original) => ({ ...await original<typeof Workspace>(), searchNames: calls.names, searchContent: calls.content }));
beforeEach(() => { calls.names.mockReset(); calls.content.mockReset(); });
afterEach(cleanup);

it('keeps the Names choice available without requiring a complete index', () => {
  show(<SearchBox state={NO_SEARCH} onChange={vi.fn()} directory="" />);
  expect(screen.getByRole('radio', { name: 'Names' })).not.toBeDisabled();
});

it('continues a live traversal even when its first time slice found no matching name', async () => {
  const source = { source: 'live' as const, as_of: '2026-11-14T00:00:00Z', observed_from: '2026-11-14T00:00:00Z' };
  calls.names.mockResolvedValueOnce({ ...source, entries: [], next_cursor: 'next', searched: 100 });
  calls.names.mockResolvedValueOnce({ ...source, entries: [{ name: 'needle', path: 'deep/needle', type: 'dir', size: 0, mtime: source.as_of, mode: 0o755, link_target: null, cached: false }], next_cursor: null, searched: 200 });
  show(<SearchResults door={{ environmentId: 'environment', threadId: null }} state={{ ...NO_SEARCH, text: 'needle' }} directory="" onOpenPath={vi.fn()} onOpenFolder={vi.fn()} />);
  const more = await screen.findByRole('button', { name: 'Continue search' });
  await userEvent.click(more);
  await waitFor(() => expect(screen.getByRole('button', { name: 'deep/needle' })).toBeInTheDocument());
  expect(calls.names.mock.calls[1]?.[3]).toBe('next');
});
