/** A known session loss is said once on the sign-in form; a first visit has no such history. */
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import type { StoreApi } from 'zustand/vanilla';
import { client, type SessionState } from '@/api/client';
import { StateLine } from '@/ui/StateLine';
import { RenewalFacts } from '@/ui/RenewalFacts';

export function SessionEndedNotice({ session = client.session }: { session?: StoreApi<SessionState> }) {
  const { t } = useTranslation();
  const ended = useStore(session, (state) => state.status === 'signed_out' && state.sessionEnded === true);
  return ended ? <StateLine role="status" compact message={t('auth.login.sessionEnded')} action={<RenewalFacts session={session} quiet />} className="text-text-muted" /> : null;
}
