/** What a person sees where an org's work waits for their second step, and what the token form sends: narrow presets as exact permissions, and a proof in the same act. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import type * as UsersApi from '@/api/users';
import { describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { proofIn } from '@/features/auth/freshProof';
import { SecondStepGate } from '@/features/auth/SecondStepGate';
import { presetScopes, TokenForm, TokenMade } from '@/features/settings/TokenForm';
import type { ApiTokenCreateBody, Me, PermissionDef } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { choiceCopy } from '@/i18n/choices';

import { show } from './support';
import { me } from '../doubles/fixtures';

const verify = vi.hoisted(() => vi.fn());
vi.mock('@/api/users', async (original) => ({
  ...(await original<typeof UsersApi>()),
  verifySecondStep: verify,
  sendEmailCode: vi.fn(() => Promise.resolve()),
}));
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ to, children }: { to: string; children: ReactNode }) => <a href={to}>{children}</a>,
}));

const ACME = { id: 'acme', kind: 'org', name: 'Acme', redacted: false };

function refusal(code: 'auth.second_step_setup' | 'auth.second_step_required', more: Record<string, unknown>): ApiError {
  const status = code === 'auth.second_step_setup' ? 403 : 401;
  return new ApiError({ type: `orch:error:${code}`, code, title: code, status, detail: code, request_id: 'r', ...more }, null);
}

function permission(name: string, level: PermissionDef['level']): PermissionDef {
  return { name, area: name.split('.')[0] ?? '', level, with: null, on: ['thread'], label: name, allows: name, profiles: false, holders: [], options_schema: null };
}

const CATALOGUE = [permission('thread.view', 'view'), permission('thread.interact', 'interact'), permission('thread.manage', 'manage'), permission('access.own', 'own')];

function withQueries(ui: ReactNode, person: Me) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(['me'], person);
  client.setQueryData(['permissions'], { permissions: CATALOGUE });
  return show(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

describe('an org that waits for a second step', () => {
  it('names the org that requires it and leads to Security while none is set up', () => {
    withQueries(<SecondStepGate error={refusal('auth.second_step_setup', { scopes: [ACME], path: '/settings/security' })} />, me(1, 'ada', 'Ada'));
    expect(screen.getByText('Set up a second step', { selector: 'p' })).toBeVisible();
    expect(screen.getByText('Acme requires a second step before its work.')).toBeVisible();
    expect(screen.getByRole('link', { name: 'Set up a second step' })).toHaveAttribute('href', '/settings/security');
  });

  it('asks this browser for a code of the step the person has, and loads the page again once it is given', async () => {
    const done = vi.fn();
    verify.mockResolvedValueOnce(me(1, 'ada', 'Ada'));
    withQueries(<SecondStepGate error={refusal('auth.second_step_required', { methods: ['totp'], scopes: [ACME] })} onDone={done} />, me(1, 'ada', 'Ada'));
    await userEvent.type(screen.getByLabelText('Code from your authenticator app'), '123456');
    await userEvent.click(screen.getByRole('button', { name: 'Verify' }));
    await waitFor(() => expect(done).toHaveBeenCalled());
    expect(verify).toHaveBeenCalledWith({ totp: '123456' });
  });
});

describe('a new token', () => {
  it('offers a saved second factor when the server says one exists, and sends it with the fresh password', async () => {
    const made = vi.fn<(body: ApiTokenCreateBody) => void>();
    withQueries(<TokenForm formId="recovered-token" onMake={made} pending={false}
      error={refusal('auth.second_step_required', { methods: ['totp', 'recovery_code'] })} />,
    me(1, 'ada', 'Ada', { totp_enabled: true }));
    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Name'), 'deploy');
    await user.type(screen.getByLabelText('Your password'), 'my own long phrase');
    await user.click(screen.getByRole('button', { name: 'Use a recovery code' }));
    await user.type(screen.getByLabelText('Recovery code', { exact: true }), '0123-4567-89AB-CDEF-GHJK');
    screen.getByRole('textbox', { name: 'Name' }).closest('form')?.requestSubmit();
    await waitFor(() => expect(made).toHaveBeenCalledOnce());
    expect(made.mock.calls[0]?.[0].proof).toEqual({ password: 'my own long phrase', recovery_code: '0123-4567-89AB-CDEF-GHJK' });
  });

  it('opens narrow and sends exactly the permissions of the chosen preset with the password', async () => {
    expect(presetScopes('view', CATALOGUE)).toEqual(['thread.view']);
    expect(presetScopes('manage', CATALOGUE)).toEqual(['thread.interact', 'thread.manage', 'thread.view']);
    const made = vi.fn<(body: ApiTokenCreateBody) => void>();
    withQueries(<TokenForm formId="t" onMake={made} pending={false} error={null} />, me(1, 'ada', 'Ada'));
    await userEvent.type(screen.getByLabelText('Name'), 'deploy');
    await userEvent.type(screen.getByLabelText('Your password'), 'my own long phrase');
    await userEvent.click(screen.getByRole('radio', { name: choiceCopy(i18n.t, 'tokenPreset.interact').name }));
    screen.getByRole('textbox', { name: 'Name' }).closest('form')?.requestSubmit();
    await waitFor(() => expect(made).toHaveBeenCalled());
    const body = made.mock.calls[0]?.[0];
    expect(body?.scopes).toEqual(['thread.interact', 'thread.view']);
    expect(body?.proof).toEqual({ password: 'my own long phrase' });
    expect(body?.expires_at).toBeDefined();
  });

  it('asks a person without a password to prove their linked sign-in again, and makes nothing before', async () => {
    const made = vi.fn();
    const person = me(1, 'ada', 'Ada', { has_password: false, identities: [{ idp_key: 'corp', idp_name: 'Corp', email: null, linked_at: '2026-10-01T00:00:00.000Z' }] });
    withQueries(<TokenForm formId="t" onMake={made} pending={false} error={null} />, person);
    expect(screen.getByText('You sign in with Corp. Prove your sign-in there again; you come back to this form as you left it.')).toBeVisible();
    expect(screen.queryByLabelText('Your password')).not.toBeInTheDocument();
    await userEvent.type(screen.getByLabelText('Name'), 'deploy');
    screen.getByRole('textbox', { name: 'Name' }).closest('form')?.requestSubmit();
    expect(await screen.findByText('Prove your sign-in first.')).toBeVisible();
    expect(made).not.toHaveBeenCalled();
  });

  it('starts the chosen lifetime when the maker submits, even after the form has stood open', async () => {
    const made = vi.fn<(body: ApiTokenCreateBody) => void>();
    withQueries(<TokenForm formId="later-token" onMake={made} pending={false} error={null} />, me(1, 'ada', 'Ada'));
    await userEvent.type(screen.getByLabelText('Name'), 'later-deploy');
    await userEvent.type(screen.getByLabelText('Your password'), 'my own long phrase');
    expect(made).not.toHaveBeenCalled();
    const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.parse('2026-11-01T10:00:00Z'));
    try {
      screen.getByRole('textbox', { name: 'Name' }).closest('form')?.requestSubmit();
    } finally {
      clock.mockRestore();
    }
    await waitFor(() => expect(made).toHaveBeenCalledOnce());
    // The initial ninety-day choice begins at submission, rather than at the earlier render.
    expect(made.mock.calls[0]?.[0].expires_at).toBe('2027-01-30T10:00:00.000Z');
  });

  it('says a creation answered again was made, and shows no secret', () => {
    withQueries(
      <TokenMade made={{ token: null, already_created: true, info: { id: 't', name: 'deploy', scopes: ['thread.view'], node: null, created_at: '2026-10-01T00:00:00.000Z', last_used_at: null, expires_at: null } }} />,
      me(1, 'ada', 'Ada'),
    );
    expect(screen.getByText('deploy was made already')).toBeVisible();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  });

  it('reads the one-use proof from the fragment and nothing else', () => {
    expect(proofIn('#identity_proof=oip_abc')).toBe('oip_abc');
    expect(proofIn('#other=1')).toBeNull();
    expect(proofIn('')).toBeNull();
  });
});
