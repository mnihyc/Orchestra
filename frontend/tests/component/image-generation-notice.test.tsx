/** Image operation limits use the shared failure notice with the supplied allowance and reset. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { FailureAlert } from '@/features/timeline/FailureAlert';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('shows the actual image allowance and reset in %s', async (locale) => {
  await setLocale(locale);
  const reset = '2026-10-12T12:35:52Z';
  const view = show(<FailureAlert error={{ code: 'image_generation_limit', message: 'Native diagnostic', retryable: false, window: 'imagegen_premium', resets_at: reset }} />);
  expect(screen.getByRole('alert')).toHaveTextContent(locale === 'en' ? 'Image generation has reached its usage limit.' : '图像生成已达到使用限额。');
  expect(screen.getByText(locale === 'en' ? 'Limit: imagegen_premium' : '限额：imagegen_premium')).toBeVisible();
  expect(screen.getByText(i18n.t('timeline.failure.resets'))).toBeVisible();
  expect(view.container.querySelector('time')).toHaveAttribute('datetime', reset);
  expect(screen.queryByText('Native diagnostic')).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('does not invent a reset for an ordinary image failure in %s', async (locale) => {
  await setLocale(locale);
  const view = show(<FailureAlert error={{ code: 'image_generation_failed', message: 'Native diagnostic', retryable: false }} />);
  expect(screen.getByRole('alert')).toHaveTextContent(locale === 'en' ? 'The image could not be generated.' : '无法生成图像。');
  expect(view.container.querySelector('time')).toBeNull();
  expect(screen.queryByText(i18n.t('timeline.failure.resets'))).not.toBeInTheDocument();
});
