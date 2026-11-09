/** The same declared access cause and owner fix appear in thread notices and provider request facts. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { FailureAlert } from '@/features/timeline/FailureAlert';
import { failureKey, sameFailure } from '@/features/timeline/failures';
import { RequestFacts } from '@/features/usage/RequestFacts';
import { i18n, setLocale } from '@/i18n';
import { requestRow } from '../doubles/request-row';
import { timingFixture } from '../doubles/performance';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });
it('joins the same structured native and terminal cause while retaining other known refusals', () => {
  const native = { code: 'model_unavailable', message: 'Unavailable', details: { reason: 'credit_program' } };
  const terminal = { code: native.code, message: native.message, reason: 'credit_program' };
  expect(sameFailure(native, terminal)).toBe(true);
  expect(failureKey(native)).toBe(failureKey(terminal));
  expect(sameFailure(native, { ...terminal, reason: 'access' })).toBe(false);
  expect(sameFailure(native, { code: native.code, message: native.message })).toBe(true);
});
for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the specific owner fix for a native item and terminal error in ${locale}`, async () => {
    await setLocale(locale);
    const value = { code: 'model_unavailable', message: 'Native source words', retryable: false };
    const view = show(<FailureAlert error={{ ...value, details: { reason: 'credit_program' } }} />);
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('ui.error.creditProgramRestricted'));
    view.rerender(<FailureAlert error={{ ...value, reason: 'credit_program' }} />);
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('ui.error.creditProgramRestricted'));
    expect(screen.getByRole('alert')).not.toHaveTextContent(i18n.t('ui.error.operationFailed'));
  });
  it(`retains the same billing cause and fix in provider request facts in ${locale}`, async () => {
    await setLocale(locale);
    const row = requestRow({ timing: timingFixture({ outcome: { state: 'told', value: 'model_unavailable' }, error_reason: 'credit_program', upstream_status: 400 }) });
    show(<RequestFacts event={row} details />);
    expect(screen.getByText(i18n.t('usageRequest.reasons.credit_program'))).toBeVisible();
    expect(screen.queryByText('credit_program')).not.toBeInTheDocument();
  });
}
