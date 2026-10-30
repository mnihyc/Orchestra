/** Saving a draft changes one small mark in the row rather than adding a line below it. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { DraftMark } from '@/features/composer/DraftMark';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { show } from './support';

const initialLocale = currentLocale();
afterEach(async () => { await setLocale(initialLocale); });
it.each(['en', 'zh-CN'] as const)('keeps one footprint and announces only the current draft state (%s)', async (locale) => {
  await setLocale(locale);
  const view = show(<DraftMark status="idle" />);
  const mark = document.querySelector('[data-draft-mark]');
  expect(mark).toHaveClass('h-(--control-sm)', 'w-3');
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  view.rerender(<DraftMark status="saving" />);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('composer.draft.saving'));
  expect(screen.getByText(i18n.t('composer.draft.saving'))).toHaveClass('sr-only');
  view.rerender(<DraftMark status="failed" />);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('composer.draft.failed'));
  expect(screen.queryByText(i18n.t('composer.draft.saving'))).not.toBeInTheDocument();
  expect(document.querySelector('[data-draft-mark]')).toBe(mark);
});
