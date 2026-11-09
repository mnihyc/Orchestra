/** Ask adapts the shared composer without losing multiline drafts or idempotent input identities. */
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AskComposer, type AskComposerProps } from '@/features/composer/AskComposer';
import { useComposerStore } from '@/features/composer/Draft';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/ask/ModelChoice', () => ({ AskModelChoice: () => null }));
vi.mock('@/features/composer/Attachments', () => ({ AttachButton: () => null, AttachmentChips: () => null, UploadChips: () => null }));
afterEach(async () => { cleanup(); useComposerStore.setState({ text: {}, parts: {} }); await setLocale('en'); });
const props = (scope: string): AskComposerProps => ({ scope, root: 'root', model: null, active: false, disabled: false,
  ensure: vi.fn(async () => 'conversation'), send: vi.fn(async () => undefined), stop: vi.fn(async () => undefined), onModel: vi.fn() });

for (const locale of ['en', 'zh-CN'] as const) it(`Enter sends, Shift+Enter and IME do not send, in ${locale}`, async () => {
  await setLocale(locale); const t = i18n.getFixedT(locale), input = props(`enter-${locale}:new`);
  show(<AskComposer {...input} />);
  const box = screen.getByRole('textbox', { name: t('ask.composer') });
  fireEvent.change(box, { target: { value: 'First line\nSecond line' } });
  fireEvent.keyDown(box, { key: 'Enter', shiftKey: true }); fireEvent.keyDown(box, { key: 'Enter', isComposing: true });
  expect(input.send).not.toHaveBeenCalled();
  fireEvent.keyDown(box, { key: 'Enter' }); await waitFor(() => expect(input.send).toHaveBeenCalledOnce());
  expect(vi.mocked(input.send).mock.calls[0]?.[0]).toEqual([{ type: 'text', text: 'First line\nSecond line' }]);
  await waitFor(() => expect(box).toHaveValue(''));
});

it('keeps a failed draft and retries it with the same client identity', async () => {
  const input = props('retry:new'); vi.mocked(input.send).mockRejectedValueOnce(new Error('The connection ended'));
  show(<AskComposer {...input} />);
  const box = screen.getByRole('textbox', { name: i18n.t('ask.composer') });
  fireEvent.change(box, { target: { value: 'Keep this question' } }); fireEvent.keyDown(box, { key: 'Enter' });
  await waitFor(() => expect(input.send).toHaveBeenCalledOnce());
  await waitFor(() => expect(box).toBeEnabled()); expect(box).toHaveValue('Keep this question');
  fireEvent.keyDown(box, { key: 'Enter' }); await waitFor(() => expect(input.send).toHaveBeenCalledTimes(2));
  expect(vi.mocked(input.send).mock.calls[0]?.[1]).toBe(vi.mocked(input.send).mock.calls[1]?.[1]);
});

it('moves a first draft to the allocated conversation and restores it after closing the view', async () => {
  const input = { ...props('allocation:new'), draftKey: 'ask:allocation-editor' }, view = show(<AskComposer {...input} />);
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Unsent before upload' } });
  view.rerender(<AskComposer {...input} scope="allocation:conversation" />);
  await waitFor(() => expect(screen.getByRole('textbox')).toHaveValue('Unsent before upload'));
  view.unmount(); show(<AskComposer {...input} scope="allocation:conversation" />);
  expect(screen.getByRole('textbox')).toHaveValue('Unsent before upload');
});

it('uses the explicit Stop operation while the answer runs', async () => {
  const input = { ...props('stop:conversation'), active: true }; show(<AskComposer {...input} />);
  fireEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') }));
  await waitFor(() => expect(input.stop).toHaveBeenCalledOnce()); expect(input.send).not.toHaveBeenCalled();
});

it('keeps Stop available independently of message access and a pending page context', async () => {
  const input = { ...props('stop-only:conversation'), active: true, disabled: true, canStop: true };
  show(<AskComposer {...input} />);
  expect(screen.getByRole('textbox')).toBeDisabled();
  const stop = screen.getByRole('button', { name: i18n.t('composer.stop') }); expect(stop).toBeEnabled(); fireEvent.click(stop);
  await waitFor(() => expect(input.stop).toHaveBeenCalledOnce()); expect(input.send).not.toHaveBeenCalled();
});
