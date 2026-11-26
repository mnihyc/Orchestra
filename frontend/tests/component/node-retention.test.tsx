/** Retention observations keep their native scope and never move a frozen recovery promise. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ArchiveConsequences } from '@/features/project/ReviewConsequences';
import { InspectionBanner } from '@/features/trace/InspectionBanner';
import type { NodeArchivePlan } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show, skipOverflowFallback } from './support';
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), Link: ({ children }: { children: React.ReactNode }) => <a href="#place">{children}</a> }));
skipOverflowFallback();
afterEach(async () => { await setLocale('en'); });
const at = '2026-11-15T10:00:00.000Z';
const plan: NodeArchivePlan = { preflight_id: 'review', stamp: 'reviewed', revision: 1, expires_at: at, archiving: true,
  node: { id: 'topic', name: 'Review', kind: 'topic', redacted: false }, subtree: [], work: [], environments: [],
  counts: { orgs: 0, projects: 0, topics: 1, threads: 0, documents: 0 }, copies_kept: 0, live_shares_kept: 0, blockers: [] };
for (const locale of ['en', 'zh-CN'] as const) {
  it(`retires the old schedule and source when the review no longer reports them (${locale})`, async () => {
    await setLocale(locale);
    const retention = { archive: { days: 3, source: 'org' as const, eligible_at: { state: 'at' as const, at }, hold_reason: null },
      delete: { days: 0, source: 'topic' as const, eligible_at: { state: 'not_applicable' as const }, hold_reason: null }, measured_at: at };
    const value = { ...plan, retention };
    const view = show(<ArchiveConsequences plan={value} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    const facts = await screen.findByRole('dialog', { name: i18n.t('common.details') });
    expect(facts).toHaveTextContent(locale === 'en' ? 'Organization policy' : '组织策略');
    expect(facts).toHaveTextContent(locale === 'en' ? 'Automatic delete off' : '自动删除已关闭');
    expect(facts.querySelector(`time[datetime="${at}"]`)).not.toBeNull();
    view.rerender(<ArchiveConsequences plan={plan} />);
    expect(facts).toHaveTextContent(locale === 'en' ? 'Not reported' : '尚未报告');
    expect(facts.querySelector('time')).toBeNull();
  });
  it(`shows a physical cleanup hold separately from the same recorded restoration cutoff (${locale})`, async () => {
    await setLocale(locale);
    const original = { deleted_at: at, deleted_by: null, purge_at: '2026-11-22T10:00:00.000Z', readonly: true,
      cleanup_hold: { held: true, measured_at: at } };
    const view = show(<InspectionBanner facts={original} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    const facts = await screen.findByRole('dialog', { name: i18n.t('common.details') });
    expect(facts).toHaveTextContent(locale === 'en' ? 'Retained dependencies' : '保留的依赖项');
    expect(facts.querySelector(`time[datetime="${original.purge_at}"]`)).not.toBeNull();
    view.rerender(<InspectionBanner facts={{ ...original, cleanup_hold: { held: false, measured_at: at } }} />);
    expect(facts).not.toHaveTextContent(locale === 'en' ? 'Retained dependencies' : '保留的依赖项');
    expect(facts.querySelector(`time[datetime="${original.purge_at}"]`)).not.toBeNull();
    expect(facts).not.toHaveTextContent(locale === 'en' ? 'Purged' : '已清除');
  });
}
