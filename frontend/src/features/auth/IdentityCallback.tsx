/** Sign-in and identity-link callback outcomes.
 *
 * A sign-in through an identity provider that did not end in a session comes back to the sign-in
 * page with a code in the address. This puts the code into words: what happened, and what to do.
 */
import { useTranslation } from 'react-i18next';

import { InlineAlert } from '@/ui/InlineAlert';

/** The codes a callback comes back with. */
const OUTCOMES = [
  'denied',
  'state_expired',
  'domain_not_allowed',
  'registration_closed',
  'invite_required',
  'link_required',
  'disabled',
  'idp_error',
] as const;

type Outcome = (typeof OUTCOMES)[number];

const isOutcome = (code: string): code is Outcome => (OUTCOMES as readonly string[]).includes(code);

export function IdentityCallback({ code }: { code: string }) {
  const { t } = useTranslation();
  // A code this build does not know is still a refusal, and is said as one.
  const outcome: Outcome = isOutcome(code) ? code : 'idp_error';
  return <InlineAlert tone="danger">{t(`auth.callback.${outcome}`)}</InlineAlert>;
}
