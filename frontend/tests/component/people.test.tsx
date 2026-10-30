/** Reader comments and discussion notifications expose focus, current choices and refused mutations. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import * as people from '@/api/people';
import { ApiError } from '@/api/client';
import { CommentField } from '@/features/people/CommentField';
import { DiscussionBell } from '@/features/people/DiscussionBell';
import { i18n, setLocale } from '@/i18n';
import { choiceCopy } from '@/i18n/choices';
import { ADA } from '../doubles/chat-rows';
import { show } from './support';
import type { Document as HeardDocument } from '@/generated/api-types';

vi.mock('@/api/people', async (original) => ({ ...(await original<typeof people>()), setDiscussionLevel: vi.fn(), documentReadersQuery: (id: string, q: string) => ({ queryKey: ['readers', id, q], queryFn: () => Promise.resolve({ items: q === '' || ADA.username.startsWith(q) ? [{ user: ADA, can_view: true }] : [], next_cursor: null }) }) }));
beforeEach(() => { vi.mocked(people.setDiscussionLevel).mockReset(); });
afterEach(async () => { await setLocale('en'); });
const doc = { id: 'document', discussion_level: 'involved' } as HeardDocument;

function Field() {
  const [text, setText] = useState('');
  return <QueryClientProvider client={new QueryClient()}><CommentField docId="document" label="New discussion" placeholder="Comment" autofocus value={text} onChange={setText} onSend={vi.fn()} /></QueryClientProvider>;
}
it('a reader’s opened comment gets focus and choosing a current reader restores the caret', async () => {
  show(<Field />); const box = screen.getByRole('textbox', { name: 'New discussion' }); expect(box).toHaveFocus();
  fireEvent.change(box, { target: { value: 'Confirm @', selectionStart: 9 } });
  const choice = await screen.findByRole('option', { name: /Ada Lovelace/ }); fireEvent.mouseDown(choice);
  await waitFor(() => expect(box).toHaveValue('Confirm @ada ')); await waitFor(() => expect(box).toHaveFocus());
  expect(await screen.findByRole('button', { name: 'Notify Ada Lovelace' })).toBeVisible();
});
it.each(['en', 'zh-CN'] as const)('the document bell keeps its shipped involved value after a refused choice in %s', async (locale) => {
  await setLocale(locale);
  const t = i18n.getFixedT(locale);
  const label = t('people.discussionBell');
  const all = choiceCopy(t, 'discussionLevel.all');
  const involved = choiceCopy(t, 'discussionLevel.involved');
  let refuse: ((reason: ApiError) => void) | undefined;
  vi.mocked(people.setDiscussionLevel).mockImplementation(() => new Promise<void>((_resolve, reject) => { refuse = reject; }));
  show(<DiscussionBell doc={doc} />);
  fireEvent.pointerDown(screen.getByRole('button', { name: label }), { button: 0, ctrlKey: false });
  const choice = await screen.findByRole('menuitemradio', { name: all.name });
  // D425 keeps the separate canonical explanation inside an open picker, not on the closed bell.
  expect(choice).toHaveAccessibleName(all.name);
  expect(choice).toHaveAccessibleDescription(all.description);
  expect(screen.getByText(all.description, { exact: true })).toBeVisible();
  expect(choice).toHaveAttribute('aria-checked', 'false');
  expect(screen.getByRole('menuitemradio', { name: involved.name })).toHaveAttribute('aria-checked', 'true');
  fireEvent.click(choice);
  await waitFor(() => expect(people.setDiscussionLevel).toHaveBeenCalledWith('document', 'all'));
  const waiting = screen.getByRole('button', { name: label });
  expect(waiting).toHaveAttribute('aria-disabled', 'true');
  expect(waiting).toHaveAttribute('aria-busy', 'true');
  expect(waiting).toHaveAccessibleDescription(t('notificationBell.level.involved'));
  fireEvent.pointerDown(waiting, { button: 0, ctrlKey: false });
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  expect(screen.queryByText(all.description, { exact: true })).not.toBeInTheDocument();
  expect(people.setDiscussionLevel).toHaveBeenCalledTimes(1);
  await act(async () => { refuse?.(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Current permission required', detail: 'Current permission required', request_id: 'component-refused', required: 'doc.view', missing: [], requestable: true, override: false }, null)); await Promise.resolve(); });
  expect(await screen.findByRole('alert')).toBeVisible();
  await waitFor(() => expect(screen.getByRole('button', { name: label })).toBeEnabled());
  expect(waiting).toHaveAccessibleDescription(t('notificationBell.level.involved'));
  fireEvent.pointerDown(screen.getByRole('button', { name: label }), { button: 0, ctrlKey: false });
  const selected = await screen.findByRole('menuitemradio', { name: involved.name });
  expect(selected).toHaveAttribute('aria-checked', 'true');
  expect(selected).toHaveAccessibleDescription(involved.description);
  expect(screen.getByRole('menuitemradio', { name: all.name })).toHaveAttribute('aria-checked', 'false');
});
