/** A changed source keeps the existing paragraph and requires a new explicit summary act. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { SummaryBox } from '@/features/summaries/SummaryBox';
import { setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['source_changed', 'content_changing', 'settings_changed'] as const)(`keeps the paragraph and waits for Summarize again when %s (${locale})`, async (reason) => {
    await setLocale(locale);
    const refresh = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);
    const error = new ApiError({ type: 'orch:error:provider.unavailable', code: 'provider.unavailable', title: '',
      status: 409, detail: 'The summary source changed.', request_id: 'source-change-case', reason }, null);
    show(<SummaryBox readerKey="source-reader" target={{ kind: 'document', reference: '/document/source' }}
      summary={{ revision: '2', state: 'stale', text: 'The previous paragraph remains.', producer: null, updated_at: null, error }}
      onRefresh={refresh} onAsk={vi.fn()} />);
    expect(screen.getByText('The previous paragraph remains.')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    const sentence = locale === 'en'
      ? { source_changed: 'Source changed while writing · Result discarded · Usage still counted', content_changing: 'Content kept changing before writing', settings_changed: 'Summary settings changed before completion' }[reason]
      : { source_changed: '撰写时来源已更改 · 结果已丢弃 · 用量仍计入统计', content_changing: '撰写前内容持续变化', settings_changed: '摘要完成前设置已更改' }[reason];
    expect(screen.getByText(sentence, { exact: false })).toBeVisible();
    expect(refresh).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: locale === 'en' ? 'Summarize again' : '重新生成摘要' }));
    expect(refresh).toHaveBeenCalledOnce();
  });
}
