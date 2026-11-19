/** Upload history keeps the actor, names, folder, measured size and outcome together. */
import { screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { UploadHistory } from '@/features/files/UploadHistory';
import type { HistoryLine } from '@/generated/api-types';
import { show } from './support';

vi.mock('@/features/people/NamedChip', () => ({ TypedChip: ({ reference }: { reference: { path: string } }) => <a href="#file">{reference.path.split('/').at(-1)}</a> }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: { display_name: string } }) => <button>{user.display_name}</button> }));

const line = {
  id: 'history', at: '2026-01-01T00:00:00Z', actor: { display_name: 'Bo' }, action: 'workspace.file_written', text: '', about: null,
  upload: { environment_id: 'workspace', folder: 'src/assets', bytes: 24, replaced: 1, outcome: 'complete', files: ['screenshot.png', 'data.csv', 'notes.txt'].map((name) => ({ name, path: `src/assets/${name}`, size: 8, received: 8, outcome: 'complete', replaced: false })) },
} as HistoryLine;

it('shows the successful batch with file chips and a compact remainder', () => {
  show(<UploadHistory line={line} />);
  expect(screen.getByRole('button', { name: 'Bo' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'screenshot.png' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'data.csv' })).toBeInTheDocument();
  expect(screen.getByText('and 1 more')).toBeInTheDocument();
  expect(screen.getByText('to src/assets')).toBeInTheDocument();
  expect(screen.getByText('· 24 B')).toBeInTheDocument();
  expect(screen.getByText('· replaced 1')).toBeInTheDocument();
});

it.each(['failed', 'cancelled'] as const)('says %s with a measured partial size and no link to an unwritten file', (outcome) => {
  show(<UploadHistory line={{ ...line, upload: { ...line.upload!, outcome, replaced: 0, bytes: 7, files: [{ name: 'partial.txt', path: null, size: 100, received: 7, outcome, replaced: false }] } }} />);
  expect(screen.getByText(outcome === 'failed' ? 'upload failed' : 'upload cancelled')).toBeInTheDocument();
  expect(screen.getByText('partial.txt')).toBeInTheDocument();
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
  expect(screen.getByText('· 7 B')).toBeInTheDocument();
});
