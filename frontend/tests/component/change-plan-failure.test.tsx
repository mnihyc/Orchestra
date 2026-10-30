/** Missing threads and saved plans retain a retry without hiding a server failure. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { PlanFailure } from '@/features/change/ThreadChange';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`explains a missing thread or saved plan and requests a new plan (${locale})`, async () => {
    await setLocale(locale);
    const retry = vi.fn();
    const error = new ApiError({ type: 'orch:error:not_found', status: 404, code: 'not_found', title: 'Not found', detail: 'The thread could not be found.', request_id: 'missing-plan' }, null);
    show(<PlanFailure error={error} onRetry={retry} />);
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('change.planGone'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('change.expired.again') }));
    expect(retry).toHaveBeenCalledOnce();
  });

  it(`keeps a server failure distinct from a missing thread or plan (${locale})`, async () => {
    await setLocale(locale);
    const retry = vi.fn();
    const error = new ApiError({ type: 'orch:error:internal', status: 500, code: 'internal', title: 'Request failed', detail: 'The database connection was interrupted.', request_id: 'server-failure' }, null);
    show(<PlanFailure error={error} onRetry={retry} />);
    expect(screen.getByRole('alert')).toBeVisible();
    expect(screen.queryByText(i18n.t('change.planGone'))).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(retry).toHaveBeenCalledOnce();
  });
}
