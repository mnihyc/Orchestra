/** Retained field facts describe the actual attempt while the person's current value stays authoritative. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import type { Node } from '@/generated/api-types';
import { AutomaticFieldFacts } from '@/features/summaries/AutomaticFieldFacts';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const subject = (): Node => ({ id: 'topic', kind: 'topic', name: 'Chosen by a person', title_source: 'user', tags_source: 'auto',
  path: [{ id: 'topic', kind: 'topic', name: 'Topic', redacted: false, thread_display: null }],
  automatic_fields: { title: { last_attempt: { outcome: 'discarded', discard_reason: 'changed_by_person', producer: { kind: 'model', title: 'Recorded model', offering: 'model', provider: null },
    usage_owner: null, captured_usage: { input_tokens: 8, output_tokens: 0, total_tokens: 8, cached_input_tokens: null, cache_write_tokens: null, reasoning_tokens: null }, result_facts: null, at: '2026-11-15T04:00:00Z' },
    schedule: { state: 'idle', reason: null, due_at: null, next_at: null, last_attempt_at: '2026-11-15T04:00:00Z', last_success_at: '2026-11-14T03:00:00Z', retry_ceiling: 90, source: { scope: 'default', node: null } } },
    tags: { last_attempt: null, schedule: null } },
} as unknown as Node);
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps the manual value and recorded zero while linking the captured default to its real control (${locale})`, async () => {
    await setLocale(locale);
    const node = subject();
    show(<><h1>{node.name}</h1><AutomaticFieldFacts node={node} field="title" /></>);
    expect(screen.getByText('Chosen by a person')).toBeVisible();
    expect(screen.getByText(locale === 'en' ? 'Result discarded' : '结果已丢弃')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryFacts.title') }));
    const dialog = await screen.findByRole('dialog', { name: i18n.t('summaryFacts.title') });
    expect(within(dialog).getByText(locale === 'en' ? 'Changed by a person' : '已由人工修改')).toBeVisible();
    expect(within(dialog).getByText(locale === 'en' ? /0 tokens/u : /0 词元/u)).toBeVisible();
    expect(within(dialog).getByRole('link', { name: i18n.t('settings.editor.from.default') })).toHaveAttribute('href', '/topic/topic?tab=settings&section=defaults&setting=summary.retry_ceiling_secs');
    expect(screen.getByText('Chosen by a person')).toBeVisible();
  });

  it(`keeps an older automatic record unreported rather than filling its times or allowance (${locale})`, async () => {
    await setLocale(locale);
    const node = { ...subject(), title_source: 'auto', automatic_fields: undefined } as Node;
    show(<AutomaticFieldFacts node={node} field="title" inline />);
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.getAllByText(locale === 'en' ? 'Not reported' : '尚未报告').length).toBeGreaterThan(0);
    expect(document.querySelector('time')).toBeNull();
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  });
}
