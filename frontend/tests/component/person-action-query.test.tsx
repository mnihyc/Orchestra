/** One bounded person-context read survives a StrictMode remount without duplicate HTTP work. */
import { useQuery } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import { StrictMode } from 'react';
import { expect, it, vi } from 'vitest';
import { personActionsQuery, type PersonActions } from '@/api/personActions';
import type * as Client from '@/api/client';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<{ data: PersonActions; response: Response }>>() }));
vi.mock('@/api/client', async (original) => ({ ...await original<typeof Client>(), api: { GET: calls.read } }));

function Probe() {
  const read = useQuery(personActionsQuery({ kind: 'document', id: 'doc' }, 'person'));
  return <span>{read.data?.can_notify === true ? 'Ready' : 'Loading'}</span>;
}

it('shares the in-flight request across a temporary observer remount', async () => {
  let finish: ((answer: { data: PersonActions; response: Response }) => void) | undefined;
  calls.read.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  show(<StrictMode><Probe /></StrictMode>);
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
  finish?.({ data: { can_view: true, can_notify: true, gives: [], note_chars: 200 }, response: new Response(null, { status: 200 }) });
  expect(await screen.findByText('Ready')).toBeVisible();
  expect(calls.read).toHaveBeenCalledTimes(1);
});
