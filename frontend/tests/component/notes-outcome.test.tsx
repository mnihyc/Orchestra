/** A conditional proposal and a captured notes outcome are different facts. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { MemoryPlanFacts, MemoryResultFacts } from '@/features/change/MemoryFacts';
import { i18n, setLocale } from '@/i18n';
import { show, skipOverflowFallback } from './support';
skipOverflowFallback();
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`keeps a target-kept receipt after a later copy proposal and withholds an unreadable directory (${locale})`, async () => {
  await setLocale(locale);
  const memory = { carried: true, files: 3, bytes: 1024, proposal: { source_directory: 'web/.orch/memory', target_directory: null,
    proposed_fate: 'copy_if_absent' as const, observed_target_revision: 4 } };
  const proposal = show(<MemoryPlanFacts memory={memory} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const body = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  expect(body).toHaveTextContent('web/.orch/memory');
  expect(body).toHaveTextContent(locale === 'en' ? 'Not shared' : '未共享');
  expect(body).not.toHaveTextContent('Copied');
  proposal.unmount();
  const at = '2026-11-15T10:00:00.000Z';
  const view = show(<MemoryResultFacts outcome={{ fate: 'target_kept', at, cleanup_pending: false }} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const result = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  expect(result).toHaveTextContent(locale === 'en' ? 'Target kept' : '保留目标笔记');
  expect(result.querySelector(`time[datetime="${at}"]`)).not.toBeNull();
  view.rerender(<MemoryResultFacts outcome={null} />);
  expect(result).toHaveTextContent(locale === 'en' ? 'Not reported' : '尚未报告');
  expect(result.querySelector('time')).toBeNull();
});
