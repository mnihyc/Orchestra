/** The one split action stops a running turn independently of its draft and sends only the delivery the person chooses. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as ThreadsApi from '@/api/threads';
import type * as ThreadState from '@/realtime/stores/thread';
import { defaultThreadRead } from '@/realtime/threadScope';
import { DeliveryControl } from '@/features/composer/DeliveryControl';
import { useStop } from '@/features/composer/StopControl';
import type { Delivery } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ interrupt: vi.fn(), send: vi.fn<(delivery: Delivery) => void>(), apply: vi.fn(), aside: vi.fn() }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), interruptTurn: calls.interrupt }));
vi.mock('@/realtime/stores/thread', async (load) => { const actual = await load<typeof ThreadState>(); return { ...actual, threadWriter: { ...actual.threadWriter, apply: calls.apply } }; });

function Harness({ active = true, empty = false, mayDrive = true, live = true, noSteer = false, waitingFiles = false, side = false, sideUnavailable = false }: { active?: boolean; empty?: boolean; mayDrive?: boolean; live?: boolean; noSteer?: boolean; waitingFiles?: boolean; side?: boolean; sideUnavailable?: string | false }) {
  const stop = useStop('thread-1', active ? 'turn-1' : null, live);
  return <DeliveryControl state={{ active, starting: false, live, cannotSteer: noSteer, agent: 'Scripted agent' }} stop={stop} turnId={active ? 'turn-1' : null}
    mayDrive={mayDrive} empty={empty} tooLong={false} attachmentsWaiting={waitingFiles} onSend={calls.send} onAskOnSide={side ? calls.aside : undefined} sideUnavailable={sideUnavailable} />;
}

beforeEach(() => { calls.send.mockClear(); calls.aside.mockClear(); calls.interrupt.mockReset(); calls.apply.mockClear(); calls.interrupt.mockResolvedValue({ id: 'turn-1', state: 'interrupted' }); });
afterEach(async () => { await setLocale('en'); });

describe('the message delivery action', () => {
  it.each(['en', 'zh-CN'] as const)('explains an attachment refusal for a supported aside and preserves normal delivery in %s', async (locale) => {
    await setLocale(locale);
    const reason = i18n.t('side.unavailable.attachments');
    const view = show(<Harness side sideUnavailable={reason} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.deliveryMenu') }));
    const aside = screen.getByRole('menuitem', { name: i18n.t('side.menu.askBox') });
    expect(aside).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByText(reason, { exact: true })).toBeVisible();
    expect(screen.getByRole('menuitem', { name: i18n.t('composer.deliveryLabel.queue') })).not.toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(aside);
    expect(calls.aside).not.toHaveBeenCalled(); expect(calls.send).not.toHaveBeenCalled();
    view.rerender(<Harness side />);
    await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('side.menu.askBox') }));
    expect(calls.aside).toHaveBeenCalledExactlyOnceWith();
  });

  it.each(['en', 'zh-CN'] as const)('offers no unsupported side action while keeping ordinary delivery in %s', async (locale) => {
    await setLocale(locale);
    show(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.deliveryMenu') }));
    expect(screen.queryByRole('menuitem', { name: i18n.t('side.menu.askBox') })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('composer.deliveryLabel.queue') }));
    expect(calls.send).toHaveBeenCalledExactlyOnceWith('queue');
    expect(calls.aside).not.toHaveBeenCalled();
  });
  it.each([false, true])('uses the ordinary small control height for the whole split action (running: %s)', (active) => {
    show(<Harness active={active} />);
    for (const button of [screen.getByRole('button', { name: active ? 'Stop' : 'Send' }), screen.getByRole('button', { name: 'More ways to send' })]) {
      expect(button).toHaveClass('h-(--control-sm)');
      expect(button).not.toHaveClass('h-(--control-md)');
      expect(button).not.toHaveClass('w-full');
    }
  });

  it('sends an idle message once with queue delivery', async () => {
    show(<Harness active={false} />);
    expect(screen.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(calls.send).toHaveBeenCalledExactlyOnceWith('queue');
    expect(calls.interrupt).not.toHaveBeenCalled();
  });

  it('disables both halves of idle Send and exposes one reason instead of a disabled menu', async () => {
    const user = userEvent.setup();
    show(<Harness active={false} empty />);
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    const more = screen.getByRole('button', { name: 'More ways to send' });
    expect(more).toBeDisabled();
    await user.click(more);
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    // A click on either disabled half focuses its focusable reason wrapper. Tab would leave
    // that wrapper; go back first, then verify an actual keyboard entry into the explanation.
    const reason = more.closest('[data-tooltip-trigger]');
    expect(reason).toHaveFocus();
    await user.tab({ shift: true });
    expect(reason).not.toHaveFocus();
    await user.tab();
    expect(reason).toHaveFocus();
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Write a message or attach a file first.');
    expect(screen.getAllByRole('tooltip')).toHaveLength(1);
    expect(calls.send).not.toHaveBeenCalled();
  });

  it('changes Send into Stop while running and stops even with an empty draft', async () => {
    show(<Harness empty />);
    expect(screen.queryByRole('button', { name: 'Send' })).not.toBeInTheDocument();
    const stop = screen.getByRole('button', { name: 'Stop' });
    expect(stop).toBeEnabled();
    await userEvent.click(stop);
    await waitFor(() => expect(calls.interrupt).toHaveBeenCalledExactlyOnceWith(defaultThreadRead('thread-1'), 'turn-1'));
    expect(calls.send).not.toHaveBeenCalled();
  });

  it('disables both halves of offline Stop with one keyboard-readable reason', async () => {
    show(<Harness live={false} />);
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'More ways to send' })).toBeDisabled();
    await userEvent.tab();
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Waiting for the connection to come back.');
    expect(screen.getAllByRole('tooltip')).toHaveLength(1);
    expect(calls.interrupt).not.toHaveBeenCalled();
  });

  it.each([['Queue', 'queue'], ['Steer now', 'steer'], ['Interrupt and send', 'interrupt']] as const)('uses %s only after it is chosen in the split menu', async (label, delivery) => {
    show(<Harness />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'More ways to send' }));
    await user.click(screen.getByRole('menuitem', { name: new RegExp(`^${label}`) }));
    expect(calls.send).toHaveBeenCalledExactlyOnceWith(delivery);
    expect(calls.interrupt).not.toHaveBeenCalled();
  });

  it('keeps the menu accessible with an empty draft and explains unavailable delivery', async () => {
    show(<Harness empty />);
    await userEvent.click(screen.getByRole('button', { name: 'More ways to send' }));
    expect(screen.getByRole('menuitem', { name: /^Queue/ })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getAllByText('Write a message or attach a file first.').length).toBeGreaterThan(0);
    expect(calls.send).not.toHaveBeenCalled();
  });

  it('preserves unavailable steering and keeps viewer and offline Stop read-only', async () => {
    const { unmount } = show(<Harness noSteer />);
    await userEvent.click(screen.getByRole('button', { name: 'More ways to send' }));
    expect(screen.getByRole('menuitem', { name: /^Steer now/ })).toHaveAttribute('aria-disabled', 'true');
    unmount();
    const offline = show(<Harness live={false} />);
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
    offline.unmount();
    show(<Harness mayDrive={false} />);
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'More ways to send' })).toBeDisabled();
    expect(calls.interrupt).not.toHaveBeenCalled();
  });
});
