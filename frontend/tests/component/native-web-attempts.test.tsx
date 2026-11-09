/** Native web failures and unproven attempts stay distinct, localized operation notices. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { FailureAlert } from '@/features/timeline/FailureAlert';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const provenFailure of [true, false]) {
  it(`keeps a ${provenFailure ? 'failed' : 'unproven'} web attempt distinct in ${locale}`, async () => {
    await setLocale(locale);
    const view = show(<FailureAlert error={{ code: provenFailure ? 'native_web_action_failed' : 'native_web_action_unproven',
      message: 'Private native diagnostic', retryable: false, details: { reason: provenFailure ? 'native_web_request_failed' : 'native_result_not_found' } }} />);
    const expected = provenFailure ? i18n.t('ui.error.nativeWebActionFailed') : i18n.t('ui.error.nativeWebActionUnproven');
    expect(screen.getByRole('alert')).toHaveTextContent(expected);
    expect(screen.queryByText('Private native diagnostic')).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('timeline.failure.resets'))).not.toBeInTheDocument();
    expect(view.container.querySelector('time')).toBeNull();
  });
}
