/** The public Help selection handed to Ask is exactly the visible, localized result set. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type { Guide } from '@/help';
import { articleInLanguage, makeGuide } from '@/help/guide';
import { HelpContent } from '@/features/help/HelpView';
import { useHelpState } from '@/features/help/state';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const host = vi.hoisted(() => ({ guide: undefined as Guide | undefined, open: vi.fn() }));
vi.mock('@/help/localized', () => ({ useLocalizedGuide: () => host.guide }));
vi.mock('@/features/ask/open', () => ({ openAskWithDraft: host.open }));
const guide = makeGuide({
  'first.md': '---\nsummary: First quartz summary.\nkeywords: quartz\n---\n# Quartz first\n\nText.',
  'second.md': '---\nsummary: Second quartz summary.\nkeywords: quartz\n---\n# Quartz second\n\nText.',
  'other.md': '---\nsummary: Unselected sentinel.\n---\n# Other\n\nText.',
}, {}, { 'zh-CN': {
  'first.md': '---\nsummary: 第一份石英摘要。\nkeywords: quartz\n---\n# 石英一\n\n正文。',
  'second.md': '---\nsummary: 第二份石英摘要。\nkeywords: quartz\n---\n# 石英二\n\n正文。',
  'other.md': '---\nsummary: 未选中的内容。\n---\n# 其他\n\n正文。',
} });
const before = useHelpState.getState();
afterEach(async () => { cleanup(); useHelpState.setState(before, true); host.open.mockClear(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`freezes the displayed public results and their summaries (${locale})`, async () => {
  await setLocale(locale);
  host.guide = { articles: guide.articles.map((article) => articleInLanguage(article, locale)) };
  useHelpState.setState({ article: null, query: 'quartz' });
  const page = show(<HelpContent />);
  await screen.findByRole('button', { name: i18n.t('summaryBox.askResults') });
  const ids = [...page.container.querySelectorAll('[data-help-result]')].map((element) => element.getAttribute('data-help-result'));
  expect(new Set(ids)).toEqual(new Set(['first', 'second']));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') }));
  expect(host.open).toHaveBeenCalledTimes(1);
  const opening = host.open.mock.calls[0]?.[0] as { text: string } | undefined;
  expect(opening?.text).toContain('/help/first');
  expect(opening?.text).toContain('/help/second');
  for (const article of host.guide.articles.filter((article) => article.id !== 'other')) expect(opening?.text).toContain(`> ${article.summary ?? ''}`);
  expect(opening?.text).not.toContain('/help/other');
  const frozen = opening?.text;
  await userEvent.clear(screen.getByRole('searchbox', { name: i18n.t('help.search.label') }));
  expect(opening?.text).toBe(frozen);
  expect(host.open).toHaveBeenCalledTimes(1);
});
