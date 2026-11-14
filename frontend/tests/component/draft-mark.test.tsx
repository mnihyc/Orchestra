/** Saving a draft changes one small mark in the row rather than adding a line below it. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as ThreadsApi from '@/api/threads';
import { useComposerStore } from '@/features/composer/Draft';
import { DraftMark } from '@/features/composer/DraftMark';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ save: vi.fn<typeof ThreadsApi.saveDraft>() }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), saveDraft: calls.save }));

const initialLocale = currentLocale();
afterEach(async () => { cleanup(); useComposerStore.setState({ text: {}, parts: {}, draftSaves: {}, dirtyDrafts: new Set() }); calls.save.mockReset(); await setLocale(initialLocale); });
it.each(['en', 'zh-CN'] as const)('keeps one footprint and announces only the current draft state (%s)', async (locale) => {
  await setLocale(locale);
  const view = show(<DraftMark status="idle" />);
  const mark = document.querySelector('[data-draft-mark]');
  expect(mark).toHaveClass('size-(--control-sm)');
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  view.rerender(<DraftMark status="saving" />);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('composer.draft.saving'));
  expect(screen.getByText(i18n.t('composer.draft.saving'))).toHaveClass('sr-only');
  view.rerender(<DraftMark status="failed" />);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('composer.draft.failed'));
  expect(screen.queryByText(i18n.t('composer.draft.saving'))).not.toBeInTheDocument();
  expect(document.querySelector('[data-draft-mark]')).toBe(mark);
});

it.each(['en', 'zh-CN'] as const)('opens the failed draft reason and retries its current unsent words (%s)', async (locale) => {
  await setLocale(locale);
  const thread = `draft-retry-${locale}`;
  useComposerStore.setState({ text: { [thread]: 'Still an unsent draft' }, parts: { [thread]: [] }, draftSaves: {
    [thread]: { threadId: thread, status: 'failed', confirmed: null, error: new Error('Draft save failed') },
  } });
  calls.save.mockResolvedValue({ content: [{ type: 'text', text: 'Still an unsent draft' }], delivery: 'queue', native_action: null, review_comments: [], updated_at: '2026-11-14T00:00:00Z' });
  function Mark() { const status = useComposerStore((state) => state.draftSaves[thread]?.status ?? 'idle'); return <DraftMark status={status} threadId={thread} />; }
  const user = userEvent.setup();
  show(<Mark />);
  await user.click(screen.getByRole('button', { name: i18n.t('composer.draft.failed') }));
  expect(screen.getByRole('alert')).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith(thread, [{ type: 'text', text: 'Still an unsent draft' }], 'queue'));
  await waitFor(() => expect(screen.queryByRole('status')).toBeNull());
  expect(useComposerStore.getState().text[thread]).toBe('Still an unsent draft');
});
