/** UTC rollup days are labelled once, independently of a person's zone, with compact shared date controls. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Period } from '@/features/usage/Period';
import { spanOf } from '@/features/usage/search';
import { currentLocale, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

const NOW = Date.parse('2026-10-15T00:30:00Z');

it.each([
  ['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark'],
] as const)('names the range and zone once, including its open calendar (%s, %s)', async (locale, theme) => {
  const previousLocale = currentLocale();
  const previous = useAppearance.getState();
  const previousTheme = document.documentElement.getAttribute('data-theme');
  await setLocale(locale);
  useAppearance.setState({ locale, timeZone: 'America/Los_Angeles' });
  document.documentElement.setAttribute('data-theme', theme);
  const view = show(<Period span={spanOf({ range: '7d' }, NOW)} onSearch={vi.fn()} now={NOW} />);
  try {
    const zone = locale === 'en' ? 'Days use UTC' : '日期按 UTC 计算';
    expect(screen.getAllByText(zone)).toHaveLength(1);
    const ranges = screen.getByRole('radiogroup', { name: locale === 'en' ? 'Span' : '时间范围' });
    expect(within(ranges).getByRole('radio', { name: locale === 'en' ? '7 days' : '7 天' })).toHaveAttribute('aria-checked', 'true');
    expect(within(ranges).getByRole('radio',{name:locale==='en'?'Last week':'上周'})).toHaveAttribute('aria-checked', 'false');
    // It is still October 14 in the person's zone. This explicitly labelled range follows UTC rollups.
    expect(view.container).toHaveTextContent(locale === 'en' ? 'Oct 9 – Oct 15 (UTC)' : '10月9日 – 10月15日（UTC）');
    expect(view.container.querySelector('[data-choice-description]')).toBeNull();
    await userEvent.setup().click(screen.getByRole('button', { name: locale === 'en' ? 'Custom…' : '自定义…' }));
    expect(screen.getAllByText(zone)).toHaveLength(1);
    const from = screen.getByLabelText(locale === 'en' ? 'From' : '从', { exact: true });
    expect(from).toHaveAccessibleDescription(new RegExp(zone));
  } finally {
    view.unmount(); await setLocale(previousLocale);
    useAppearance.setState({ locale: previous.locale, timeZone: previous.timeZone });
    if (previousTheme === null) document.documentElement.removeAttribute('data-theme');
    else document.documentElement.setAttribute('data-theme', previousTheme);
  }
});
