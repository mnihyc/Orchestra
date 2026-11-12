/** Source actions keep the current permission and missing-data state at their existing control. */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as SourceApi from '@/api/source';
import type * as ThreadStore from '@/realtime/stores/thread';
import { CommentOnLines } from '@/features/source/Review';
import { ThreadLinks } from '@/features/source/Links';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ thread: null as Thread | null, save: vi.fn<typeof SourceApi.saveThreadLinks>(), apply: vi.fn() }));
vi.mock('@/realtime/stores/thread', async (original) => {
  const actual = await original<typeof ThreadStore>();
  return { ...actual, useThreadRow: () => calls.thread, threadWriter: { ...actual.threadWriter, apply: calls.apply } };
});
vi.mock('@/api/source', async (original) => ({ ...await original<typeof SourceApi>(), saveThreadLinks: calls.save }));
const patch = '--- a/main.ts\n+++ b/main.ts\n@@ -1 +1 @@\n-old\n+new\n';
function thread(interact: boolean, archived = false): Thread {
  return { id: 'thread', name: 'Source review', links: [], archived, access: { permissions: interact ? ['thread.interact'] : [], admin_only: [] } } as unknown as Thread;
}
beforeEach(() => { vi.clearAllMocks(); calls.thread = thread(true); calls.save.mockResolvedValue([]); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['missing', 'permission', 'no-lines'] as const)(`keeps the review control and explains %s (${locale})`, async (kind) => {
    await setLocale(locale);
    calls.thread = kind === 'missing' ? null : thread(kind !== 'permission');
    const user = userEvent.setup();
    show(<CommentOnLines threadId="thread" path="main.ts" diff={kind === 'no-lines' ? '' : patch} itemId={null} />);
    const button = screen.getByRole('button', { name: i18n.t('source.review.comment') });
    expect(button).toHaveAttribute('aria-disabled', 'true');
    await user.hover(button);
    const reason = kind === 'missing' ? 'threadUnavailable' : kind === 'permission' ? 'threadInteract' : 'noChangedLines';
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t(`implicitAvailability.${reason}`));
    await user.click(button);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it.each([false, true])(`keeps Edit links on an empty readonly thread (archived=%s, ${locale})`, async (archived) => {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<ThreadLinks thread={thread(false, archived)} />);
    const button = screen.getByRole('button', { name: i18n.t('source.links.edit') });
    expect(button).toHaveAttribute('aria-disabled', 'true');
    await user.hover(button);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t(archived ? 'implicitAvailability.archived' : 'implicitAvailability.threadInteract'));
    await user.click(button);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(calls.save).not.toHaveBeenCalled();
  });

  it(`keeps an authored review draft when Interact ends instead of silently closing it (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const view = show(<CommentOnLines threadId="thread" path="main.ts" diff={patch} itemId={null} />);
    await user.click(screen.getByRole('button', { name: i18n.t('source.review.comment') }));
    const dialog = screen.getByRole('dialog');
    const text = within(dialog).getByRole('textbox', { name: i18n.t('source.review.text') });
    await user.click(text); await user.paste('Keep this review');
    calls.thread = thread(false);
    view.rerender(<CommentOnLines threadId="thread" path="main.ts" diff={patch} itemId={null} />);
    expect(text).toHaveValue('Keep this review');
    const save = within(dialog).getByRole('button', { name: i18n.t('source.review.save') });
    expect(save).toHaveAttribute('aria-disabled', 'true');
    expect(save).toHaveAttribute('aria-description', i18n.t('implicitAvailability.threadInteract'));
    await user.click(save);
    expect(dialog).toBeVisible();
  });

  it(`keeps historical link inspection readonly even with a retained Interact grant (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<ThreadLinks thread={thread(true)} readOnly />);
    const edit = screen.getByRole('button', { name: i18n.t('source.links.edit') });
    expect(edit).toHaveAttribute('aria-disabled', 'true');
    await user.hover(edit);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('composer.row.readOnly'));
    await user.click(edit);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it(`keeps a link draft while permission is lost and permits its explicit save after access returns (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const view = show(<ThreadLinks thread={thread(true)} />);
    await user.click(screen.getByRole('button', { name: i18n.t('source.links.edit') }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('source.links.title') });
    await user.click(within(dialog).getByRole('button', { name: i18n.t('source.links.another') }));
    await user.click(within(dialog).getByRole('textbox', { name: i18n.t('source.links.label') }));
    await user.paste('Kept review');
    await user.click(within(dialog).getByRole('textbox', { name: i18n.t('source.links.url') }));
    await user.paste('https://example.test/review');
    view.rerender(<ThreadLinks thread={thread(false)} />);
    expect(within(dialog).getByRole('textbox', { name: i18n.t('source.links.url') })).toHaveValue('https://example.test/review');
    const save = within(dialog).getByRole('button', { name: i18n.t('source.links.save') });
    expect(save).toHaveAttribute('aria-disabled', 'true');
    expect(save).toHaveAttribute('aria-description', i18n.t('implicitAvailability.threadInteract'));
    await user.click(save);
    const form = dialog.querySelector('form');
    if (form === null) throw new Error('the native links editor has a form');
    fireEvent.submit(form);
    expect(calls.save).not.toHaveBeenCalled();
    view.rerender(<ThreadLinks thread={thread(true)} />);
    const currentSave = within(dialog).getByRole('button', { name: i18n.t('source.links.save') });
    expect(currentSave).not.toHaveAttribute('aria-disabled', 'true');
    await user.click(currentSave);
    await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith('thread', [{ label: 'Kept review', url: 'https://example.test/review' }]));
  });
}
