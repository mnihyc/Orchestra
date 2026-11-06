/** A committed bounded receipt is acknowledgement, never a request to resend or disclose its payload. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ReceiptAcknowledged } from '@/api/client';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { errorText } from '@/ui/errorText';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const receipt = () => new ReceiptAcknowledged({ projection: 'receipt', id: 'accepted', secret: 'one-time-owned-sentinel' }, 201, 'receipt-request');
afterEach(async () => { vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`describes successful receipt metadata without retryability or exposing one-time content (${locale})`, async () => {
    await setLocale(locale);
    const text = errorText(receipt(), i18n.t, { locale, timeZone: 'UTC' });
    expect(text.acknowledged).toBe(true);
    expect(text.retryable).toBe(false);
    expect(text.status).toBe(201);
    expect(text.requestId).toBe('receipt-request');
    expect(text.message).toBe(i18n.t('ui.receipt.accepted'));
    expect(text.message).not.toContain('one-time-owned-sentinel');
  });

  it(`replaces failure actions with explicit bounded readback after an acknowledgement (${locale})`, async () => {
    await setLocale(locale);
    const resend = vi.fn(); const read = vi.fn();
    const view = show(<ErrorState error={receipt()} title="Failed save" message="Connection lost" onRetry={resend} onReadback={read}
      action={<button onClick={resend}>Send again</button>} recovery={{ label: 'Retry operation', content: <button onClick={resend}>Replay</button> }} />);
    expect(view.container.querySelector('[data-state-line]')).toHaveAttribute('role', 'status');
    expect(view.container.querySelector('[data-state-line]')).toHaveAttribute('data-state-tone', 'neutral');
    expect(screen.getByText(i18n.t('ui.receipt.title'))).toBeVisible();
    expect(screen.queryByText('Failed save')).not.toBeInTheDocument();
    expect(screen.queryByText('Connection lost')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Send again|Retry operation|Replay/ })).not.toBeInTheDocument();
    expect(view.container).not.toHaveTextContent('one-time-owned-sentinel');
    expect(read).not.toHaveBeenCalled();
    expect(resend).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('ui.receipt.readback') }));
    expect(read).toHaveBeenCalledOnce();
    expect(resend).not.toHaveBeenCalled();
  });

  it(`keeps accepted status without inventing a read or rendering a mutation retry (${locale})`, async () => {
    await setLocale(locale);
    const resend = vi.fn();
    const view = show(<ErrorAlert error={receipt()} message="Please resend" action={<button onClick={resend}>Send again</button>} />);
    expect(view.container.querySelector('[data-state-line]')).toHaveAttribute('role', 'status');
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.queryByText('Please resend')).not.toBeInTheDocument();
    expect(resend).not.toHaveBeenCalled();
    expect(view.container).not.toHaveTextContent('one-time-owned-sentinel');
  });
}
