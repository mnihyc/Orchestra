/** A failed returned image uses the ordinary localized operation notice, without a fake plan limit. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { FailureAlert } from '@/features/timeline/FailureAlert';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('names an unretainable returned image in %s', async (locale) => {
  await setLocale(locale);
  const view = show(<FailureAlert error={{ code: 'image_result_unavailable', message: 'Native diagnostic', retryable: false,
    details: { reason: 'invalid_base64' } }} />);
  expect(screen.getByRole('alert')).toHaveTextContent(locale === 'en' ? 'The returned image could not be retained.' : '无法保留返回的图像。');
  expect(screen.queryByText('Native diagnostic')).not.toBeInTheDocument();
  expect(screen.queryByText(i18n.t('timeline.failure.resets'))).not.toBeInTheDocument();
  expect(view.container.querySelector('time')).toBeNull();
});
