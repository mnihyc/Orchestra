/** Retained output expands in the page; a refused read remains retryable without opening another shell. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { TerminalScreen } from '@/features/inspector/TerminalScreen';
import type { TerminalSession } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  read: vi.fn<(id: string) => Promise<Uint8Array | null>>(),
  ports: vi.fn<(environment: string | null, enabled: boolean) => null>(() => null),
  connect: vi.fn(),
}));
vi.mock('@/api/terminals', () => ({ shellRecording: calls.read }));
vi.mock('@/features/ports/LocalAddresses', () => ({ useLocalAddresses: calls.ports, localLinks: () => [] }));
vi.mock('@/realtime/terminal', () => ({ connectShell: calls.connect }));
vi.mock('@xterm/xterm', () => { throw new Error('Terminal code unavailable'); });

const person = me(1, 'ada', 'Ada');
const shell: TerminalSession = { id: 'shell', environment_id: 'environment', thread_id: 'thread', opened_by: userRef(person),
  attached: false, busy: false, close_reason: 'exited', closed_at: '2026-10-15T12:00:00Z', closes_at: null, cols: 80, rows: 24,
  cwd: 'web', detached_at: null, exit_code: 3, last_io_at: '2026-10-15T12:00:00Z', opened_at: '2026-10-15T11:00:00Z', recorded: true, shell: 'sh', state: 'closed' };
afterEach(async () => { cleanup(); vi.clearAllMocks(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) {
  it(`retains the output and the read's recovery path in ${locale}/${theme}`, async () => {
    await setLocale(locale);
    document.documentElement.setAttribute('data-theme', theme);
    const output = Array.from({ length: 12 }, (_, index) => `Output ${index + 1}`).join('\n');
    calls.read.mockRejectedValueOnce(new Error('Recording unavailable')).mockResolvedValueOnce(new TextEncoder().encode(output));
    const user = userEvent.setup();
    show(<TerminalScreen session={shell} firstTicket={null} threadId="thread" environmentName="Workspace" active onConnection={() => undefined} />);
    expect(calls.read).not.toHaveBeenCalled();
    expect(calls.connect).not.toHaveBeenCalled();
    expect(calls.ports).toHaveBeenLastCalledWith('environment', false);
    const read = screen.getByRole('button', { name: i18n.t('terminal.closed.show') });
    await user.click(read);
    expect(await screen.findByRole('alert')).toBeVisible();
    await user.click(read);
    const preview = await screen.findByTestId('last-printed');
    expect(within(preview).queryByText(/Output 12/)).not.toBeInTheDocument();
    const all = within(preview).getByRole('button', { name: i18n.t('ui.preview.showAll') });
    all.focus(); await user.keyboard('{Enter}');
    expect(within(preview).getByText(/Output 12/)).toBeVisible();
    expect(calls.read).toHaveBeenCalledTimes(2);
    expect(calls.connect).not.toHaveBeenCalled();
  });
}

it('offers retry when the terminal code fails instead of leaving an endless skeleton', async () => {
  const user = userEvent.setup();
  show(<TerminalScreen session={{ ...shell, state: 'open', closed_at: null, close_reason: null, exit_code: null }} firstTicket={null} threadId="thread" environmentName="Workspace" active onConnection={() => undefined} />);
  expect(await screen.findByRole('alert')).toBeVisible();
  const retry = screen.getByRole('button', { name: i18n.t('common.tryAgain') });
  await user.click(retry);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('common.tryAgain') })).toBeEnabled();
  expect(calls.connect).not.toHaveBeenCalled();
});
