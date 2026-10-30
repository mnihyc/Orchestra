/** A comment can reach later mention candidates without losing its text or caret. */
import { screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as People from '@/api/people';
import { CommentField } from '@/features/people/CommentField';
import { DORA, GRACE } from '../doubles/chat-rows';
import { show } from './support';

const pages = vi.hoisted(() => ({ read: vi.fn<(q: string, cursor: string | null) => Promise<People.DocumentMentionPage>>() }));
vi.mock('@/api/people', async (load) => {
  const actual = await load<typeof People>();
  return { ...actual, documentReadersQuery: (id: string, q = '', cursor: string | null = null) => ({ ...actual.documentReadersQuery(id, q, cursor), queryFn: () => pages.read(q, cursor), retry: false }) };
});
vi.mock('@/api/personActions', () => ({
  personActionsQuery: () => ({ queryKey: ['person-actions-test'], queryFn: () => Promise.resolve({ can_view: true, can_notify: true, gives: [], note_chars: 200 }) }),
  notifyPerson: vi.fn(), grantPerson: vi.fn(),
}));
vi.mock('@/api/profile', () => ({ profileQuery: () => ({ queryKey: ['profile-test'], queryFn: () => Promise.resolve({ is_me: false, can_message: false }) }) }));
vi.mock('@/features/profile/CardDetails', () => ({ CardDetails: () => <div>Full profile</div> }));

function Field() {
  const [text, setText] = useState('');
  return <CommentField docId="doc" label="Comment" value={text} placeholder="Comment" onChange={setText} onSend={vi.fn()} />;
}

beforeEach(() => pages.read.mockReset());
afterEach(() => vi.unstubAllGlobals());

it('inspects a highlighted person and returns to the same comment caret', async () => {
  pages.read.mockResolvedValue({ items: [{ user: DORA, can_view: true }], next_cursor: null });
  const input = userEvent.setup(); show(<Field />);
  const box = screen.getByRole('textbox', { name: 'Comment' });
  await input.type(box, '@');
  await screen.findByRole('option', { name: /Dora Developer/ });
  await input.keyboard('{ArrowRight}');
  expect(await screen.findByRole('dialog', { name: 'Dora Developer' })).toHaveTextContent('Full profile');
  await input.keyboard('{Escape}');
  await waitFor(() => expect(box).toHaveFocus());
  expect(box).toHaveValue('@');
  await input.keyboard('{Enter}');
  expect(box).toHaveValue('@dora ');
});

it('keeps touch choices uncovered and opens the profile from Inspect', async () => {
  vi.stubGlobal('matchMedia', () => ({ matches: true, media: '(hover: none)', addEventListener: vi.fn(), removeEventListener: vi.fn() }));
  pages.read.mockResolvedValue({ items: [{ user: DORA, can_view: true }], next_cursor: 'second' });
  const input = userEvent.setup(); show(<Field />);
  const box = screen.getByRole('textbox', { name: 'Comment' });
  await input.type(box, '@');
  expect(await screen.findByRole('option', { name: 'More people' })).toBeVisible();
  expect(document.querySelector('[data-inspection-card]')).toBeNull();
  await input.click(await screen.findByRole('button', { name: 'Inspect Dora Developer' }));
  expect(await screen.findByRole('dialog', { name: 'Dora Developer' })).toHaveTextContent('Full profile');
});

it('loads the next page by keyboard and keeps the caret until a person is chosen', async () => {
  pages.read.mockImplementation((_q: string, cursor: string | null) => Promise.resolve(cursor === null
    ? { items: [{ user: DORA, can_view: true }], next_cursor: 'second' }
    : { items: [{ user: GRACE, can_view: false }], next_cursor: null }));
  const input = userEvent.setup(); show(<Field />);
  const box = screen.getByRole('textbox', { name: 'Comment' });
  await input.type(box, '@');
  await screen.findByRole('option', { name: /Dora Developer/ });
  await input.keyboard('{ArrowDown}{Enter}');
  const grace = await screen.findByRole('option', { name: /Grace Hopper/ });
  expect(grace).toHaveTextContent('No access');
  expect(box).toHaveValue('@');
  expect(box).toHaveFocus();
  await input.keyboard('{Enter}');
  expect(box).toHaveValue('@grace ');
  expect(await screen.findByText("Grace Hopper can't open this document")).toBeVisible();
  await input.click(screen.getByRole('button', { name: 'Leave' }));
  await input.type(box, '@');
  expect(await screen.findByRole('option', { name: 'More people' })).toBeVisible();
  expect(screen.queryByRole('option', { name: /Grace Hopper/ })).not.toBeInTheDocument();
});

it('drops a late page after the query changes', async () => {
  let finish: ((value: People.DocumentMentionPage) => void) | undefined;
  pages.read.mockImplementation((q: string, cursor: string | null) => {
    if (q !== '') return Promise.resolve({ items: [], next_cursor: null });
    if (cursor === null) return Promise.resolve({ items: [{ user: DORA, can_view: true }], next_cursor: 'second' });
    return new Promise<People.DocumentMentionPage>((resolve) => { finish = resolve; });
  });
  const input = userEvent.setup(); show(<Field />);
  const box = screen.getByRole('textbox', { name: 'Comment' });
  await input.type(box, '@');
  await screen.findByRole('option', { name: /Dora Developer/ });
  await input.keyboard('{ArrowDown}{Enter}');
  await screen.findByRole('option', { name: /Loading/ });
  await input.type(box, 'x');
  await waitFor(() => expect(pages.read).toHaveBeenCalledWith('x', null));
  finish?.({ items: [{ user: GRACE, can_view: false }], next_cursor: null });
  await waitFor(() => expect(screen.queryByRole('option', { name: /Grace Hopper/ })).not.toBeInTheDocument());
  expect(box).toHaveValue('@x');
});

it('offers a retry for a failed next page without clearing the draft', async () => {
  let attempts = 0;
  pages.read.mockImplementation((_q, cursor) => {
    if (cursor === null) return Promise.resolve({ items: [{ user: DORA, can_view: true }], next_cursor: 'second' });
    attempts += 1;
    return attempts === 1 ? Promise.reject(new Error('offline')) : Promise.resolve({ items: [{ user: GRACE, can_view: true }], next_cursor: null });
  });
  const input = userEvent.setup(); show(<Field />);
  const box = screen.getByRole('textbox', { name: 'Comment' });
  await input.type(box, '@');
  await screen.findByRole('option', { name: /Dora Developer/ });
  await input.keyboard('{ArrowDown}{Enter}');
  await screen.findByRole('option', { name: /Could not load people · Try again/ });
  expect(box).toHaveValue('@');
  await input.keyboard('{Enter}');
  expect(await screen.findByRole('option', { name: /Grace Hopper/ })).toBeVisible();
  expect(attempts).toBe(2);
});

it('keeps one candidate page after repeated requests in a million-person directory', async () => {
  const total = 1_000_000, size = 50;
  pages.read.mockImplementation((_query, cursor) => {
    const offset = cursor === null ? 0 : Number(cursor);
    return Promise.resolve({ items: Array.from({ length: Math.min(size, total - offset) }, (_, index) => {
      const number = String(offset + index).padStart(6, '0');
      return { user: { ...DORA, id: `person-${number}`, username: `person-${number}`, display_name: `Person ${number}` }, can_view: true };
    }), next_cursor: offset + size < total ? String(offset + size) : null });
  });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const input = userEvent.setup(); show(<QueryClientProvider client={cache}><Field /></QueryClientProvider>);
  await input.type(screen.getByRole('textbox', { name: 'Comment' }), '@');
  await screen.findByRole('option', { name: /Person 000000/ });
  expect(pages.read).toHaveBeenCalledTimes(1);
  for (let page = 1; page <= 20; page += 1) {
    await input.click(screen.getByRole('option', { name: 'More people' }));
    await screen.findByRole('option', { name: new RegExp(`Person ${String(page * size).padStart(6, '0')}`) });
    expect(screen.getAllByRole('option')).toHaveLength(size + 1);
    await waitFor(() => expect(cache.getQueryCache().findAll({ queryKey: ['document-readers', 'doc'] })).toHaveLength(1));
    expect(screen.queryByRole('option', { name: /Person 000000/ })).not.toBeInTheDocument();
  }
  expect(pages.read).toHaveBeenCalledTimes(21);
  await input.click(screen.getByRole('button', { name: 'First people' }));
  expect(await screen.findByRole('option', { name: /Person 000000/ })).toBeVisible();
  expect(screen.getAllByRole('option')).toHaveLength(size + 1);
}, 30_000); // Twenty separate bounded page interactions; structural assertions, not loaded-machine timing, prove scale.
