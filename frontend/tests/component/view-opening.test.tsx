/** Kept private views report visible openings once, preserving drafts and suppressing hidden updates. */
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useLayoutEffect, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { useAdminOpening } from '@/features/access/opening';
import { clearViewOpenings, forgetViewOpening, openingForView, showViewOpening, ViewOpening } from '@/features/workbench/viewOpening';
import { beginOpening, openingId } from '@/lib/opening';

interface Announcement { body: { kind: string; id: string; opening_id: string }; headers: Record<string, string> }
const calls = vi.hoisted(() => ({ post: vi.fn<(path: string, options: Announcement) => Promise<unknown>>() }));
vi.mock('@/api/client', () => ({ api: { POST: calls.post }, done: async (request: Promise<unknown>) => { await request; } }));

const VIEW = 'kept-document-opening';
function Slot({ route }: { route: string }) {
  useLayoutEffect(() => showViewOpening(VIEW, route), [route]);
  return null;
}
function Body({ id, reached }: { id: string; reached: boolean }) {
  useAdminOpening('document', id, reached);
  const [draft, setDraft] = useState('');
  return <input aria-label="Draft" value={draft} onChange={(event) => setDraft(event.target.value)} />;
}
function Kept({ shown, route, position = 'main', id = 'private-document', reached = true }: { shown: boolean; route: string; position?: string; id?: string; reached?: boolean }) {
  return <><ViewOpening view={VIEW} route={route}><Body id={id} reached={reached} /></ViewOpening>{shown && <Slot key={position} route={route} />}</>;
}
async function settle() { await act(async () => { await Promise.resolve(); }); }

beforeEach(() => { beginOpening(); calls.post.mockReset(); calls.post.mockResolvedValue(undefined); });
afterEach(() => { cleanup(); forgetViewOpening(VIEW); });

it('announces a cached reopening with a fresh nonce and keeps its draft', async () => {
  const route = openingId();
  const view = render(<Kept shown={false} route={route} />);
  await settle();
  expect(calls.post).not.toHaveBeenCalled();
  view.rerender(<Kept shown route={route} />);
  await waitFor(() => expect(calls.post).toHaveBeenCalledTimes(1));
  expect(calls.post).toHaveBeenLastCalledWith('/me/admin-openings', { body: { kind: 'document', id: 'private-document', opening_id: route }, headers: { 'x-orch-opening': route } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Draft' }), { target: { value: 'Work still here' } });
  view.rerender(<Kept shown={false} route={route} />);
  await settle();
  view.rerender(<Kept shown route={route} />);
  await waitFor(() => expect(calls.post).toHaveBeenCalledTimes(2));
  const reopened = calls.post.mock.calls[1]?.[1];
  expect(reopened?.body.opening_id).not.toBe(route);
  expect(reopened?.headers['x-orch-opening']).toBe(reopened?.body.opening_id);
  expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveValue('Work still here');
});

it('keeps a visible move in the same opening and ignores hidden data changes', async () => {
  const route = openingId();
  const view = render(<Kept shown route={route} />);
  await waitFor(() => expect(calls.post).toHaveBeenCalledTimes(1));
  view.rerender(<Kept shown route={route} position="bottom" />);
  await settle();
  expect(calls.post).toHaveBeenCalledTimes(1);
  view.rerender(<Kept shown={false} route={route} />);
  await settle();
  view.rerender(<Kept shown={false} route={route} id="another-private-document" />);
  await settle();
  expect(calls.post).toHaveBeenCalledTimes(1);
});

it('keeps the displayed route while navigation is pending, then shares the committed request nonce', async () => {
  const route = openingId();
  const view = render(<Kept shown route={route} />);
  await waitFor(() => expect(calls.post).toHaveBeenCalledTimes(1));
  beginOpening();
  const next = openingId();
  view.rerender(<Kept shown route={route} />);
  await settle();
  expect(calls.post).toHaveBeenCalledTimes(1);
  view.rerender(<Kept shown route={next} id="next-private-document" />);
  await waitFor(() => expect(calls.post).toHaveBeenCalledTimes(2));
  expect(calls.post).toHaveBeenLastCalledWith('/me/admin-openings', { body: { kind: 'document', id: 'next-private-document', opening_id: next }, headers: { 'x-orch-opening': next } });
});

it('does not report a view reached through ordinary access', async () => {
  render(<Kept shown route={openingId()} reached={false} />);
  await settle();
  expect(calls.post).not.toHaveBeenCalled();
});

it('forgets a previous reader’s draft and admission until the next reader opens that view', async () => {
  const route = openingId();
  const view = render(<Kept shown route={route} />);
  await waitFor(() => expect(calls.post).toHaveBeenCalledTimes(1));
  fireEvent.change(screen.getByRole('textbox', { name: 'Draft' }), { target: { value: 'Previous reader draft' } });
  act(() => { clearViewOpenings(); });
  expect(screen.queryByRole('textbox', { name: 'Draft' })).toBeNull();
  // A late cleanup from the old slot must not retire a replacement admission.
  beginOpening();
  const next = openingId();
  view.rerender(<Kept shown={false} route={next} id="next-reader-document" />);
  await settle();
  expect(calls.post).toHaveBeenCalledTimes(1);
  view.rerender(<Kept shown route={next} id="next-reader-document" />);
  await waitFor(() => expect(calls.post).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveValue('');
  expect(calls.post).toHaveBeenLastCalledWith('/me/admin-openings', { body: { kind: 'document', id: 'next-reader-document', opening_id: next }, headers: { 'x-orch-opening': next } });
});

it('uses an independent UUID for each persisted view, preserving it on a move and retiring it on close', () => {
  const first = openingForView('local-tab-name');
  expect(first).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i);
  expect(openingForView('local-tab-name')).toBe(first);
  expect(openingForView('another-local-tab')).not.toBe(first);
  forgetViewOpening('local-tab-name');
  expect(openingForView('local-tab-name')).not.toBe(first);
  clearViewOpenings();
});
