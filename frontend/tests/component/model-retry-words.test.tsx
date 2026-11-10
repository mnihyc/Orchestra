/** Native retry counts are formatted without inventing one for an unknown or malformed report. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { ModelRetryWords } from '@/features/timeline/ModelRetryWords';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { show } from './support';

const before=useAppearance.getState();
afterEach(async()=>{useAppearance.setState(before);await setLocale('en');});
for(const locale of ['en','zh-CN'] as const) {
  for(const attempt of [1,4,1234]) it(`shows the supplied native retry ${attempt} (${locale})`,async()=>{
    await setLocale(locale);useAppearance.setState({locale});
    show(<div role="status"><ModelRetryWords attempt={attempt} /></div>);
    expect(screen.getByRole('status')).toHaveTextContent(i18n.t('timeline.status.retryingAttempt',{attempt:formatNumber(attempt,locale)}));
    expect(screen.getByRole('status')).not.toHaveTextContent('Generating');
  });
  for(const attempt of [undefined,null,0,-1,1.5,Number.NaN,'4']) it(`omits an unreported native retry count ${String(attempt)} (${locale})`,async()=>{
    await setLocale(locale);useAppearance.setState({locale});
    show(<div role="status"><ModelRetryWords attempt={attempt} /></div>);
    expect(screen.getByRole('status').textContent).toBe(i18n.t('timeline.status.retrying'));
  });
}
