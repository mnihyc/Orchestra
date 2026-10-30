/** Recovery questions open the real authored heading, including translated headings, without repeating an entry. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { CallHelp } from '@/features/calls/Help';
import { loadGuide } from '@/help';
import { articleInLanguage } from '@/help/guide';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => calls.open }));
afterEach(async () => { calls.open.mockClear(); await setLocale('en'); });

const destinations = [
  ['rules', 'set-rules', '设置规则'],
  ['candidates', 'choose-candidates', '选择候选项'],
  ['recovery', 'during-recovery', '恢复过程中'],
  ['guard', 'stop-repeated-failures', '停止重复失败'],
  ['automatic', 'automatic-work-and-access', '自动工作与权限'],
  ['services', 'summaries-and-descriptions', '摘要与描述'],
] as const;

for (const locale of ['en', 'zh-CN'] as const) for (const [section, english, chinese] of destinations) {
  it(`opens the ${section} section of the shipped article (${locale})`, async () => {
    await setLocale(locale);
    const source = (await loadGuide()).articles.find((article) => article.id === 'model-call-recovery');
    if (source === undefined) throw new Error('recovery help must ship with the screen');
    const article = articleInLanguage(source, locale);
    const expected = locale === 'en' ? english : article.headings.find((heading) => heading.text === chinese)?.id;
    if (expected === undefined) throw new Error('the authored Chinese section must have its own declared destination');
    expect(article.headings.some((heading) => heading.id === expected)).toBe(true);
    expect(article.screens).toContain('/thread/$threadId');
    const view = show(<CallHelp section={section} />);
    const question = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t(`calls.help.${section}.title`) }) });
    await waitFor(() => expect(question).toHaveAttribute('data-help-heading', expected));
    await userEvent.click(question);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.more') }));
    expect(calls.open).toHaveBeenCalledExactlyOnceWith('model-call-recovery', expected);
    view.unmount();
  });
}

it('keeps one visible recovery question and restores it when the first leaves', async () => {
  const label = i18n.t('help.hint.label', { term: i18n.t('calls.help.recovery.title') });
  const first = show(<CallHelp section="recovery" />);
  show(<CallHelp section="recovery" />);
  await waitFor(() => expect(screen.getAllByRole('button', { name: label })).toHaveLength(1));
  first.unmount();
  expect(await screen.findByRole('button', { name: label })).toBeVisible();
});
