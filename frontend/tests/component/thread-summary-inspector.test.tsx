/** A retained source explains its live-summary boundary without loading another continuation. */
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import type * as Live from '@/realtime/live';
import type { ResourceSummaryProps } from '@/features/summaries/ResourceSummary';
import { ThreadSummaryInspector } from '@/features/summaries/ThreadSummaryInspector';
import { i18n, setLocale } from '@/i18n';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { resultThreadRead } from '@/realtime/threadScope';
import { threadWriter } from '@/realtime/stores/thread';
import { A, B, projection, snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const host = vi.hoisted(() => ({ live: vi.fn<(props: ResourceSummaryProps) => void>(), retry: vi.fn<typeof Live.reloadThread>() }));
vi.mock('@/realtime/live', async (loadOriginal) => ({ ...await loadOriginal<object>(), reloadThread: host.retry }));
vi.mock('@/features/summaries/ResourceSummary', () => ({ ResourceSummary: (props: ResourceSummaryProps) => {
  host.live(props); return <span>Live paragraph canary</span>;
} }));
afterEach(async () => { cleanup(); threadWriter.clear(); host.live.mockClear(); host.retry.mockClear(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`explains the retained continuation on the page without mounting a live summary (${locale})`, async () => {
    await setLocale(locale);
    const sync = snapshot(projection(B, A, 4, false)), id = sync.thread.id;
    const read = resultThreadRead(id, 'retained-summary-inspector');
    selectThreadContinuation(read, B); threadWriter.reset(read, sync, false, captureThreadRead(read));
    show(<ThreadReadProvider read={read}><ThreadSummaryInspector threadId={id} /></ThreadReadProvider>);
    // EN: Retained page beside Parameters → compact historical-summary reason, with no live paragraph or paid act.
    // 中文：历史版本页与参数并排 → 一行摘要边界说明，不加载当前段落，也不请求付费操作。
    const reason = await screen.findByText(i18n.t('summaryBox.historical'));
    expect(reason.closest('[data-state-line]')).not.toBeNull();
    expect(host.live).not.toHaveBeenCalled();
    expect(screen.queryByText('Live paragraph canary')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.refresh') })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.ask') })).not.toBeInTheDocument();
  });
  it(`keeps loading, a failed scoped read and closed access separate from history (${locale})`, async () => {
    await setLocale(locale);
    const id = snapshot().thread.id, read = resultThreadRead(id, 'pending-summary-inspector');
    selectThreadContinuation(read, B);
    show(<ThreadReadProvider read={read}><ThreadSummaryInspector threadId={id} /></ThreadReadProvider>);
    expect(await screen.findByText(i18n.t('summaryBox.reading'))).toBeVisible();
    expect(screen.queryByText(i18n.t('summaryBox.historical'))).not.toBeInTheDocument();
    act(() => threadWriter.failed(read, new Error('Private source and audience canary')));
    expect(await screen.findByText(i18n.t('summaryBox.viewUnavailable'))).toBeVisible();
    expect(screen.queryByText('Private source and audience canary')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(host.retry).toHaveBeenCalledExactlyOnceWith(read);
    act(() => threadWriter.closed(read, 'revoked'));
    expect(screen.getByText(i18n.t('summaryBox.viewUnavailable'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
    expect(host.live).not.toHaveBeenCalled();
  });
  it(`keeps the existing live summary for a ready current continuation (${locale})`, async () => {
    await setLocale(locale);
    const sync = snapshot(projection(A, A, 4, true)), id = sync.thread.id;
    threadWriter.reset(id, sync, false, captureThreadRead(id));
    show(<ThreadSummaryInspector threadId={id} />);
    expect(await screen.findByText('Live paragraph canary')).toBeVisible();
    expect(host.live).toHaveBeenCalled();
    expect(screen.queryByText(i18n.t('summaryBox.historical'))).not.toBeInTheDocument();
  });
}
