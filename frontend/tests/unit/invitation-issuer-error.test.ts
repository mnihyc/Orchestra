/** An invitation's withdrawn authority has a distinct, localized recovery. */
import { afterAll, beforeAll, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { createElement } from 'react';

import { ApiError, invitationIssuerAuthorityLost } from '@/api/client';
import { isSpent } from '@/features/auth/Invitation';
import { callbackCode, IdentityCallback } from '@/features/auth/IdentityCallback';
import { i18n, setLocale, startI18n } from '@/i18n';
import { errorText } from '@/ui/errorText';
import { ErrorAlert } from '@/ui/ErrorState';

beforeAll(async () => { await startI18n('en'); });
afterAll(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('explains the maker’s lost authority in %s without blaming the recipient', async (locale) => {
  await setLocale(locale);
  const t = i18n.getFixedT(locale);
  for (const [code, status] of [['gone', 410], ['authz.denied', 403], ['registration.invite_required', 403]] as const) {
    const error = new ApiError({ type: `orch:error:${code}`, code, title: '', detail: '', status, request_id: '',
      reason: 'invitation_issuer_authority_lost' }, null);
    expect(invitationIssuerAuthorityLost(error)).toBe(true);
    expect(errorText(error, t, { locale }).message).toBe(t('auth.invite.issuerAuthorityLost'));
    expect(isSpent(error)).toBe(false);
  }
  const expired = new ApiError({ type: 'orch:error:gone', code: 'gone', title: '', detail: '', status: 410, request_id: '' }, null);
  expect(isSpent(expired)).toBe(true);
  expect(invitationIssuerAuthorityLost(expired)).toBe(false);
  expect(callbackCode('invite_required', 'invitation_issuer_authority_lost')).toBe('invitation_issuer_authority_lost');
  expect(callbackCode('invite_required', undefined)).toBe('invite_required');
  const denied = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: '',
    status: 403, request_id: '', reason: 'invitation_issuer_authority_lost', missing: [{
      object: { kind: 'project', id: '00000000-0000-0000-0000-000000000000', name: null, owner: null },
      permissions: ['project.view'], options: null, ask: [], ask_more: 0, override: false, closed: null,
    }] }, null);
  render(createElement(ErrorAlert, { error: denied }));
  expect(screen.getByRole('alert')).toHaveTextContent(t('auth.invite.issuerAuthorityLost'));
  expect(screen.queryByRole('button', { name: t('ui.error.reviewAccess') })).toBeNull();
  cleanup();
  render(createElement(IdentityCallback, { code: 'invitation_issuer_authority_lost' }));
  expect(screen.getByRole('alert')).toHaveTextContent(t('auth.callback.invitation_issuer_authority_lost'));
  cleanup();
});
