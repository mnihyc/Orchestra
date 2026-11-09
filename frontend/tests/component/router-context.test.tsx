/** Names remain real links inside the shared component frame, including newly introduced group headings. */
import { Link } from '@tanstack/react-router';
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`draws a native router link without replacing its semantics (${locale})`, async () => {
  await setLocale(locale);
  const name = locale === 'en' ? 'Open Inbox' : '打开收件箱';
  show(<Link to="/inbox">{name}</Link>);
  expect(screen.getByRole('link', { name })).toHaveAttribute('href', '/inbox');
});
