/** The same identity-provider entrance for sign-in and explicit registration workflows. */
import { KeyRound } from 'lucide-react';
import { useState, type MouseEvent } from 'react';
import { ErrorAlert } from '@/ui/ErrorState';
import { useTranslation } from 'react-i18next';

import { BASE_PATH } from '@/api/client';
import type { MetaIdentityProvider } from '@/generated/api-types';
import { buttonClasses } from '@/ui/Button';

export function identityStart(key: string, redirect: string | null, invitation: string | null = null, acceptInvitation = false, acceptedTermsHash?: string): string {
  const query = new URLSearchParams();
  if (redirect !== null) query.set('redirect', redirect);
  if (invitation !== null) query.set('invitation', invitation);
  if (acceptInvitation) query.set('accept_invitation', 'true');
  if (acceptInvitation && acceptedTermsHash !== undefined) query.set('accepted_terms_hash', acceptedTermsHash);
  const suffix = query.size === 0 ? '' : `?${query.toString()}`;
  return `${BASE_PATH}/auth/oidc/${encodeURIComponent(key)}/start${suffix}`;
}

export function IdentityProviders({ providers, redirect, invitation = null, acceptInvitation = false, registration = false, acceptedTermsHash, blocked = false, beforeStart }: {
  providers: readonly MetaIdentityProvider[];
  redirect: string | null;
  invitation?: string | null;
  acceptInvitation?: boolean;
  registration?: boolean;
  acceptedTermsHash?: string;
  blocked?: boolean;
  beforeStart?: () => Promise<boolean>;
}) {
  const { t } = useTranslation();
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const start = async (event: MouseEvent<HTMLAnchorElement>, href: string) => {
    if (blocked || starting) { event.preventDefault(); return; }
    if (beforeStart === undefined) return;
    event.preventDefault(); setStarting(true); setError(null);
    try { if (await beforeStart()) window.location.assign(href); }
    catch (failure) { setError(failure); }
    finally { setStarting(false); }
  };
  const available = registration ? providers.filter((provider) => provider.auto_register) : providers;
  if (available.length === 0) return null;
  return <div className="flex flex-col gap-2">
    {available.map((provider) => <a key={provider.key}
      href={blocked || starting ? undefined : identityStart(provider.key, redirect, invitation, acceptInvitation, acceptedTermsHash)}
      role="link" aria-disabled={blocked || starting || undefined} tabIndex={0}
      onClick={(event) => void start(event, identityStart(provider.key, redirect, invitation, acceptInvitation, acceptedTermsHash))}
      className={`${buttonClasses({ variant: 'secondary', block: true })} aria-disabled:cursor-not-allowed aria-disabled:opacity-50`}>
      <KeyRound aria-hidden />{t('auth.login.continueWith', { name: provider.name })}
    </a>)}
    {error !== null && <ErrorAlert error={error} />}
  </div>;
}
