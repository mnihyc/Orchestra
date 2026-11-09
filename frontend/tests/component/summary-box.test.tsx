/** The summary view preserves content and delegates only explicit acts in either language. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { SummaryBox } from '@/features/summaries/SummaryBox';
import type { SummaryAskContext, SummarySnapshot, SummaryTarget } from '@/features/summaries/model';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const target: SummaryTarget = { kind: 'thread', reference: '/thread/review' };
const snapshot = (changes: Partial<SummarySnapshot> = {}): SummarySnapshot => ({
  revision: '1', state: 'fresh', text: 'The review is ready', producer: { kind: 'extension', name: 'Rules' }, updated_at: null, ...changes,
});
function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['thread', 'topic', 'document', 'chat', 'help', 'search_results'] as const)(`keeps a %s summary explicit and seeds Ask (${locale})`, async (kind) => {
    await setLocale(locale);
    const onRefresh = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);
    const onAsk = vi.fn<(context: SummaryAskContext) => void>();
    const summary = snapshot();
    const page = { ...target, kind };
    show(<SummaryBox readerKey="preview-reader" target={page} summary={summary} onRefresh={onRefresh} onAsk={onAsk} />);
    expect(screen.getByRole('heading', { name: i18n.t(kind === 'chat' ? 'summaryBox.catchUp' : 'summaryBox.title') })).toBeVisible();
    expect(screen.getByText(i18n.t('summaryBox.state.fresh'))).toBeVisible();
    expect(screen.getByText(summary.text ?? '')).toBeVisible();
    expect(onRefresh).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.ask') }));
    expect(onAsk).toHaveBeenCalledExactlyOnceWith({ target: page, summary: summary.text });
    expect(onRefresh).not.toHaveBeenCalled();
  });

  it(`retains the paragraph during a single pending refresh and keeps Ask available (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const waiting = deferred();
    const onRefresh = vi.fn(() => waiting.promise);
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot()} onRefresh={onRefresh} onAsk={vi.fn()} />);
    const refresh = screen.getByRole('button', { name: i18n.t('summaryBox.refresh') });
    await user.click(refresh);
    expect(refresh).toHaveAttribute('aria-busy', 'true');
    expect(screen.getByText(i18n.t('summaryBox.state.updating'))).toBeVisible();
    expect(screen.getByText('The review is ready')).toBeVisible();
    expect(screen.getByRole('button', { name: i18n.t('summaryBox.ask') })).toBeEnabled();
    await user.click(refresh);
    expect(onRefresh).toHaveBeenCalledTimes(1);
    await act(async () => { waiting.resolve(); await waiting.promise; });
    view.rerender(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot({ revision: '2', text: 'The review was accepted' })} onRefresh={onRefresh} onAsk={vi.fn()} />);
    expect(screen.getByText('The review was accepted')).toBeVisible();
    expect(screen.queryByText('The review is ready')).not.toBeInTheDocument();
  });

  it(`shows an empty updating state without inventing a paragraph (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot({ state: 'updating', text: null })} onRefresh={vi.fn()} onAsk={vi.fn()} />);
    expect(view.container.querySelector('[data-summary-text]')).toBeNull();
    expect(view.container.querySelector('section')).toHaveAttribute('aria-busy', 'true');
    expect(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') })).toHaveAttribute('aria-disabled', 'true');
  });

  it(`keeps a failed paragraph and retries only when asked (${locale})`, async () => {
    await setLocale(locale);
    const onRefresh = vi.fn<() => Promise<void>>().mockRejectedValueOnce(new Error('A synthetic refusal')).mockResolvedValue(undefined);
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot()} onRefresh={onRefresh} onAsk={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    await waitFor(() => expect(view.container.querySelector('section')).toHaveAttribute('data-summary-state', 'failed'));
    expect(screen.getByRole('alert')).toBeVisible();
    expect(screen.getByText('The review is ready')).toBeVisible();
    expect(onRefresh).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(onRefresh).toHaveBeenCalledTimes(2);
  });

  it.each([null, 'The previous summary'])(`leaves a page askable without a summarizer and keeps setup explicit (${locale}, %s)`, async (text) => {
    await setLocale(locale);
    const onAsk = vi.fn<(context: SummaryAskContext) => void>();
    const onConfigure = vi.fn();
    const onRefresh = vi.fn();
    show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot({ state: 'no_summarizer', producer: null, text })} onRefresh={onRefresh} onConfigure={onConfigure} onAsk={onAsk} />);
    expect(screen.getByText(i18n.t('summaryBox.state.no_summarizer'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.refresh') })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.ask') }));
    expect(onAsk).toHaveBeenCalledExactlyOnceWith({ target, summary: text });
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.choose') }));
    expect(onConfigure).toHaveBeenCalledOnce();
    expect(onRefresh).not.toHaveBeenCalled();
  });

  it(`explains a refused refresh without dispatch and gives readers no settings act (${locale})`, async () => {
    await setLocale(locale);
    const onRefresh = vi.fn();
    const reason = locale === 'en' ? 'Provider access required' : '需要提供方使用权限';
    show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot()} refreshBlockedReason={reason} onRefresh={onRefresh} onAsk={vi.fn()} />);
    const button = screen.getByRole('button', { name: i18n.t('summaryBox.refresh') });
    expect(button).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(button);
    await userEvent.hover(button);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(reason);
    expect(onRefresh).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.choose') })).not.toBeInTheDocument();
  });

  it(`shows an admitted model through ResourceChip and a known timestamp (${locale})`, async () => {
    await setLocale(locale);
    const at = '2026-11-08T03:00:00Z';
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot({ updated_at: at, producer: {
      kind: 'model', providerId: 'provider', model: { model_name: 'small', display_name: 'Small model', parameters: {} },
    } })} onAsk={vi.fn()} />);
    expect(view.container.querySelector('.resource-chip')).toHaveTextContent('Small model');
    expect(view.container.querySelector('time')).toHaveAttribute('datetime', at);
  });

  it(`never offers model refresh for written help (${locale})`, async () => {
    await setLocale(locale);
    const onRefresh = vi.fn();
    show(<SummaryBox readerKey="preview-reader" target={{ kind: 'help', reference: '/help/getting-started' }} summary={snapshot({ producer: { kind: 'written' } })} onRefresh={onRefresh} onAsk={vi.fn()} />);
    expect(screen.getByText(i18n.t('summaryBox.written'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.refresh') })).not.toBeInTheDocument();
    expect(onRefresh).not.toHaveBeenCalled();
  });

  it(`keeps an outdated paragraph distinct from work that is actually updating (${locale})`, async () => {
    await setLocale(locale);
    const onRefresh = vi.fn();
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot({ state: 'stale' })} onRefresh={onRefresh} onAsk={vi.fn()} />);
    expect(screen.getByText(i18n.t('summaryBox.state.stale'))).toBeVisible();
    expect(screen.getByText('The review is ready')).toBeVisible();
    expect(view.container.querySelector('section')).toHaveAttribute('aria-busy', 'false');
    expect(onRefresh).not.toHaveBeenCalled();
  });

  it(`does not call a missing paragraph fresh while a summarizer is configured (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot({ state: 'empty', text: null })} onRefresh={vi.fn()} onAsk={vi.fn()} />);
    expect(screen.getByText(i18n.t('summaryBox.empty'))).toBeVisible();
    expect(screen.queryByText(i18n.t('summaryBox.state.fresh'))).not.toBeInTheDocument();
    expect(view.container.querySelector('[data-summary-text]')).toBeNull();
  });

  it(`renders a supplied failure without disclosing an absent producer (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot({ state: 'failed', producer: null })} onAsk={vi.fn()} />);
    expect(screen.getByText(i18n.t('summaryBox.state.failed'))).toBeVisible();
    expect(screen.getByRole('alert')).toBeVisible();
    expect(screen.getByText('The review is ready')).toBeVisible();
    expect(view.container.querySelector('.resource-chip')).toBeNull();
    expect(screen.queryByText('Rules')).not.toBeInTheDocument();
  });

  it(`drops an old page's pending failure when the target changes (${locale})`, async () => {
    await setLocale(locale);
    const waiting = deferred();
    const onAsk = vi.fn<(context: SummaryAskContext) => void>();
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot()} onRefresh={() => waiting.promise} onAsk={onAsk} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    const next: SummaryTarget = { kind: 'document', reference: '/doc/next' };
    view.rerender(<SummaryBox readerKey="preview-reader" target={next} summary={snapshot({ text: 'A different document' })} onAsk={onAsk} />);
    await act(async () => { waiting.reject(new Error('An old request failed')); await waiting.promise.catch(() => undefined); });
    expect(screen.getByText('A different document')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.ask') }));
    expect(onAsk).toHaveBeenCalledExactlyOnceWith({ target: next, summary: 'A different document' });
  });

  it(`lets a newer durable revision supersede an older failed request (${locale})`, async () => {
    await setLocale(locale);
    const waiting = deferred();
    const view = show(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot()} onRefresh={() => waiting.promise} onAsk={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    view.rerender(<SummaryBox readerKey="preview-reader" target={target} summary={snapshot({ revision: '2', text: 'Newly delivered summary' })} onAsk={vi.fn()} />);
    await act(async () => { waiting.reject(new Error('The earlier request failed')); await waiting.promise.catch(() => undefined); });
    expect(view.container.querySelector('section')).toHaveAttribute('data-summary-state', 'fresh');
    expect(screen.getByText('Newly delivered summary')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it(`isolates readers of the same chat even when their revision strings match (${locale})`, async () => {
    await setLocale(locale);
    const waiting = deferred();
    const chat: SummaryTarget = { kind: 'chat', reference: '/chat/shared' };
    const onAsk = vi.fn<(context: SummaryAskContext) => void>();
    const view = show(<SummaryBox readerKey="first-reader" target={chat} summary={snapshot()} onRefresh={() => waiting.promise} onAsk={onAsk} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    view.rerender(<SummaryBox readerKey="second-reader" target={chat} summary={snapshot({ text: 'The second reader’s catch-up' })} onAsk={onAsk} />);
    await act(async () => { waiting.reject(new Error('The first reader’s request failed')); await waiting.promise.catch(() => undefined); });
    expect(view.container.querySelector('section')).toHaveAttribute('data-summary-state', 'fresh');
    expect(screen.getByText('The second reader’s catch-up')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByText('The review is ready')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.ask') }));
    expect(onAsk).toHaveBeenCalledExactlyOnceWith({ target: chat, summary: 'The second reader’s catch-up' });
  });
}
