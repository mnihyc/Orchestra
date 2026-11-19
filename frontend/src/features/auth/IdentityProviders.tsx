/** The same identity-provider entrance for sign-in and explicit registration workflows. */
import { KeyRound } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { BASE_PATH } from '@/api/client';
import type { MetaIdentityProvider } from '@/generated/api-types';
import { buttonClasses } from '@/ui/Button';

export function identityStart(key: string, redirect: string | null, invitation: string | null = null, acceptInvitation = false): string {
  const query = new URLSearchParams();
  if (redirect !== null) query.set('redirect', redirect);
  if (invitation !== null) query.set('invitation', invitation);
  if (acceptInvitation) query.set('accept_invitation', 'true');
  const suffix = query.size === 0 ? '' : `?${query.toString()}`;
  return `${BASE_PATH}/auth/oidc/${encodeURIComponent(key)}/start${suffix}`;
}

export function IdentityProviders({ providers, redirect, invitation = null, acceptInvitation = false, registration = false }: {
  providers: readonly MetaIdentityProvider[];
  redirect: string | null;
  invitation?: string | null;
  acceptInvitation?: boolean;
  registration?: boolean;
}) {
  const { t } = useTranslation();
  const available = registration ? providers.filter((provider) => provider.auto_register) : providers;
  if (available.length === 0) return null;
  return <div className="flex flex-col gap-2">
    {available.map((provider) => <a key={provider.key}
      href={identityStart(provider.key, redirect, invitation, acceptInvitation)}
      className={buttonClasses({ variant: 'secondary', block: true })}>
      <KeyRound aria-hidden />{t('auth.login.continueWith', { name: provider.name })}
    </a>)}
  </div>;
}
