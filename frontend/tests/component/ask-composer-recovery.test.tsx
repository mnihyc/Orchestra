/** The one error-row recovery repeats its failed act and never silently substitutes allocation or paid work. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AskComposer, type AskComposerProps } from '@/features/composer/AskComposer';
import { useComposerStore } from '@/features/composer/Draft';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

vi.mock('@/features/ask/ModelChoice', () => ({ AskModelChoice: () => null }));
vi.mock('@/features/composer/Attachments', () => ({ AttachButton: () => null, AttachmentChips: () => null, UploadChips: () => null }));
const appearance = useAppearance.getState();
const props = (scope: string): AskComposerProps => ({ scope, root: 'root', conversationId: 'conversation', model: null, active: false, disabled: false,
  ensure: vi.fn(() => Promise.resolve('conversation')), send: vi.fn(() => Promise.resolve()), stop: vi.fn(() => Promise.resolve()), onModel: vi.fn() });
afterEach(async () => { cleanup(); useComposerStore.setState({ text: {}, parts: {} }); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`Try again repeats the failed Send with its original body/client identity and protects an edited draft (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const input = props(`send-recovery-${locale}-${theme}`); let accept: (() => void) | undefined;
    vi.mocked(input.send).mockRejectedValueOnce(new Error('The accepted answer was lost')).mockImplementationOnce(() => new Promise<void>((resolve) => { accept = resolve; }));
    show(<AskComposer {...input} />); const box = screen.getByRole('textbox', { name: i18n.t('ask.composer') });
    fireEvent.change(box, { target: { value: 'Send the original question' } }); fireEvent.keyDown(box, { key: 'Enter' });
    const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
    const original = vi.mocked(input.send).mock.calls[0];
    fireEvent.change(box, { target: { value: 'Keep this later edit unsent' } });
    retry.focus(); fireEvent.click(retry);
    await waitFor(() => expect(input.send).toHaveBeenCalledTimes(2));
    expect(vi.mocked(input.send).mock.calls[1]).toEqual(original); expect(input.ensure).not.toHaveBeenCalled();
    expect(retry).toHaveAttribute('aria-busy', 'true'); expect(retry).toHaveAttribute('aria-disabled', 'true'); expect(retry).toHaveFocus();
    fireEvent.click(retry); expect(input.send).toHaveBeenCalledTimes(2);
    await act(() => { accept?.(); return Promise.resolve(); });
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(box).toHaveValue('Keep this later edit unsent');
  });

  it(`Try again repeats Stop, independently of message authority, without allocating or sending (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const input = { ...props(`stop-recovery-${locale}-${theme}`), active: true, disabled: true, canStop: true };
    vi.mocked(input.stop).mockRejectedValueOnce(new Error('The stop answer was lost'));
    show(<AskComposer {...input} />);
    fireEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') }));
    fireEvent.click(await screen.findByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(input.stop).toHaveBeenCalledTimes(2));
    expect(input.ensure).not.toHaveBeenCalled(); expect(input.send).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  });

  it(`Try again only allocates a failed automatic draft destination and never sends (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const input = { ...props(`allocation-recovery-${locale}-${theme}`), conversationId: null };
    vi.mocked(input.ensure).mockRejectedValueOnce(new Error('Allocation failed'));
    show(<AskComposer {...input} />); const box = screen.getByRole('textbox', { name: i18n.t('ask.composer') });
    fireEvent.change(box, { target: { value: 'This is still an unsent draft' } });
    fireEvent.click(await screen.findByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(input.ensure).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(input.send).not.toHaveBeenCalled(); expect(input.stop).not.toHaveBeenCalled(); expect(box).toHaveValue('This is still an unsent draft');
  });
}

it('a late background allocation failure cannot replace the later explicit failed Send recovery', async () => {
  const input = { ...props('late-allocation-recovery'), conversationId: null }; let failAllocation: ((error: Error) => void) | undefined;
  vi.mocked(input.ensure).mockImplementationOnce(() => new Promise<string>((_resolve, reject) => { failAllocation = reject; }));
  vi.mocked(input.send).mockRejectedValueOnce(new Error('The explicit send failed'));
  show(<AskComposer {...input} />); const box = screen.getByRole('textbox', { name: i18n.t('ask.composer') });
  fireEvent.change(box, { target: { value: 'Send this question deliberately' } }); await waitFor(() => expect(input.ensure).toHaveBeenCalledOnce());
  fireEvent.keyDown(box, { key: 'Enter' }); await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  await act(() => { failAllocation?.(new Error('The earlier background allocation failed')); return Promise.resolve(); });
  fireEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(input.send).toHaveBeenCalledTimes(2));
  expect(vi.mocked(input.send).mock.calls[1]).toEqual(vi.mocked(input.send).mock.calls[0]); expect(input.ensure).toHaveBeenCalledOnce();
});

it('keeps a failed action in its own draft scope and respects current message authority on recovery', async () => {
  const first = props('recovery-scope-a'), second = { ...props('recovery-scope-b'), root: 'other-root', conversationId: 'other-conversation' };
  vi.mocked(first.send).mockRejectedValueOnce(new Error('The first send failed'));
  const view = show(<AskComposer {...first} />);
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'First scope question' } }); fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
  await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  view.rerender(<AskComposer {...second} />); expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  view.rerender(<AskComposer {...first} disabled />);
  const retry = screen.getByRole('button', { name: i18n.t('common.tryAgain') }); expect(retry).toBeDisabled();
  fireEvent.click(retry); expect(first.send).toHaveBeenCalledOnce(); expect(second.send).not.toHaveBeenCalled(); expect(first.ensure).not.toHaveBeenCalled();
  view.rerender(<AskComposer {...first} />); fireEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(first.send).toHaveBeenCalledTimes(2)); expect(second.send).not.toHaveBeenCalled();
});
