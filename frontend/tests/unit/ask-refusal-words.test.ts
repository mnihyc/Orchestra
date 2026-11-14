/** A refused Ask says why: each reason the API names has its own sentence in both languages. */
import { afterAll, beforeAll, expect, it } from 'vitest';

import { ApiError } from '@/api/client';
import { i18n, setLocale, startI18n, type TranslationKey } from '@/i18n';
import { errorText } from '@/ui/errorText';

const reasons = [
  'not_in_list', 'model_disabled', 'provider_unavailable', 'no_interface', 'options_refused', 'model_not_allowed',
  'provider_not_allowed', 'payer_unset', 'no_use', 'identity_not_usable', 'use_excludes', 'payer_no_use', 'payer_excludes',
  'no_keeper', 'keeper_offline', 'cap_reached', 'other_organization', 'root_refused',
] as const;

const refusal = (reason?: string) => new ApiError({
  type: 'orch:error:authz.denied', code: 'authz.denied', title: 'You do not have access', detail: 'You do not have access to this',
  status: 403, request_id: 'keep-for-logs', ...(reason === undefined ? {} : { reason }),
}, null);

beforeAll(async () => { await startI18n('en'); });
afterAll(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`gives every reason of a refused question its own sentence in ${locale}`, async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const generic = t('errors.authz.denied');
    for (const reason of reasons) {
      const message = errorText(refusal(reason), t, { locale }).message;
      expect(message).toBe(t(`errors.ask.${reason}` as TranslationKey));
      expect(message).not.toBe(generic);
    }
  });

  it(`keeps the generic sentence for a refusal that names no reason it knows in ${locale}`, async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    expect(errorText(refusal(), t, { locale }).message).toBe(t('errors.authz.denied'));
    expect(errorText(refusal('a_reason_nobody_wrote'), t, { locale }).message).toBe(t('errors.authz.denied'));
  });
}
