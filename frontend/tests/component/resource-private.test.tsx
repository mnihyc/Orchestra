/** Readable resources stay ordinary rows; legacy hidden references never become an administration panel. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Box } from 'lucide-react';

import { privateRowsOf, resourceRows } from '@/api/listed';
import { ResourceListFrame as ResourceList } from '@/features/resources/ResourceList';
import type { HiddenItem } from '@/generated/api-types';

import { show } from './support';

const openAccess = vi.hoisted(() => vi.fn());
vi.mock('@/features/access/AccessDialog', () => ({ openAccess }));
vi.mock('@/features/resources/trail', () => ({ rememberList: vi.fn(), useResourceHeading: vi.fn() }));
vi.mock('@/features/resources/ResourceNav', () => ({ ResourceNav: () => null }));

interface Row { id: string; name: string; owner: null; access?: { permissions: string[]; participating: boolean } }
function List({ rows, onParticipation }: { rows: Row[]; onParticipation?: (value: boolean) => void }) {
  return <ResourceList kind="providers" search={{}} title="Providers" icon={Box} lead="Available providers" label="Providers"
    load={{ isPending: false, isError: false, error: null, isRefetching: false, refetch: vi.fn() }} all={rows} rows={rows}
    onParticipation={onParticipation}
    columns={[{ id: 'name', header: 'Name', cell: (row) => <a href={`#${row.id}`}>{row.name}</a> }]} rowKey={(row) => row.id}
    text="" onText={vi.fn()} searchLabel="Find providers" searchPlaceholder="Find providers" empty={{ icon: <Box />, title: 'No providers' }} noMatch="No matching providers" />;
}

const hidden: HiddenItem = { id: 'private-provider', parent_id: 'org', kind: 'provider', name: 'Private provider', owner: null, content_hidden: true, private: true };

describe('private resources', () => {
  it('keeps a private name out of status columns and usable pickers', () => {
    const data = resourceRows<{ id: string; name: string; status: string }>([
      { id: 'readable', name: 'Readable provider', status: 'ready', content_hidden: false }, hidden,
    ]);
    expect(data.map((row) => row.status)).toEqual(['ready']);
    expect(privateRowsOf(data)).toEqual([hidden]);
  });

  it('shows ordinary readable rows without the retired private panel or a self-grant entrance', () => {
    openAccess.mockClear();
    const rows = resourceRows<Row>([{ id: 'readable', name: 'Readable provider', owner: null, content_hidden: false }, hidden]);
    show(<List rows={rows} />);
    expect(screen.getByRole('link', { name: 'Readable provider' })).toBeVisible();
    expect(screen.queryByRole('region', { name: 'Private' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Private provider' })).not.toBeInTheDocument();
    expect(screen.queryByText('Private provider')).not.toBeInTheDocument();
    expect(openAccess).not.toHaveBeenCalled();
  });

  it('delegates ordinary focus to the pager rather than filtering a served browser slice', async () => {
    const focus = vi.fn();
    show(<List onParticipation={focus} rows={[{ id: 'mine', name: 'Mine', owner: null, access: { permissions: [], participating: true } }, { id: 'other', name: 'Other', owner: null, access: { permissions: [], participating: false } }]} />);
    await userEvent.click(screen.getByRole('button', { name: 'Filter' }));
    await userEvent.click(screen.getByRole('switch', { name: 'Everything I can see' }));
    expect(focus).toHaveBeenCalledWith(true);
    expect(screen.getByRole('link', { name: 'Mine' })).toBeVisible();
    expect(screen.getByRole('link', { name: 'Other' })).toBeVisible();
  });
});
