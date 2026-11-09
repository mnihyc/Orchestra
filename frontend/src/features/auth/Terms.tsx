/** Consent belongs to the exact native text, invitation and reader; changed text needs a new choice. */
import { useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { ApiError, client } from '@/api/client';
import type { InvitationPreview, Member } from '@/generated/api-types';
import { Checkbox } from '@/ui/Checkbox';
import { Markdown } from '@/ui/Markdown';
import { RelativeTime } from '@/ui/RelativeTime';

export interface InvitationTerms { text: string; hash: string }
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
/** An absent optional value means no terms; a malformed offered value never does. */
export function invitationTerms(preview: unknown): InvitationTerms | null | undefined {
  if (!record(preview)) return undefined;
  const value: unknown = preview.terms;
  if (value === null || value === undefined) return null;
  return record(value) && typeof value.text === 'string' && typeof value.hash === 'string' && /^[a-f0-9]{64}$/u.test(value.hash)
    ? { text: value.text, hash: value.hash } : undefined;
}
export function termsRefused(error: unknown): error is ApiError {
  return error instanceof ApiError && error.status === 409 && error.code === 'invitation.terms_not_accepted';
}

export function useInvitationTerms(token: string | null, preview: InvitationPreview | undefined, initiallyAccepted: string | null = null) {
  const version = useStore(client.session, (state) => state.readerVersion);
  const scope = JSON.stringify([token, version]);
  const active = useRef(scope);
  useLayoutEffect(() => { active.current = scope; }, [scope]);
  const [accepted, setAccepted] = useState({ scope, hash: initiallyAccepted });
  const base = invitationTerms(preview);
  const basis = JSON.stringify(base);
  const [replacement, setReplacement] = useState<{ scope: string; basis: string | undefined; value: InvitationTerms | null | undefined } | null>(null);
  const terms = replacement?.scope === scope && replacement.basis === basis ? replacement.value : base;
  const checked = terms !== undefined && terms !== null && accepted.scope === scope && accepted.hash === terms.hash;
  const ready = token === null || preview !== undefined && terms !== undefined && (terms === null || checked);
  const hash = checked ? terms.hash : undefined;
  const replace = (value: InvitationTerms | null | undefined) => { if (active.current !== scope) return; setReplacement({ scope, basis, value }); setAccepted({ scope, hash: null }); };
  return { terms, checked, ready, hash,
    setChecked: (on: boolean) => setAccepted({ scope, hash: on ? terms?.hash ?? null : null }),
    handleError: (error: unknown) => { if (!termsRefused(error)) return false; replace(invitationTerms(error.problem)); return true; },
    review: (latest: InvitationPreview) => {
      if (active.current !== scope) return false;
      const next = invitationTerms(latest);
      if (next === undefined || next?.hash !== terms?.hash || next?.text !== terms?.text) { replace(next); return false; }
      return ready;
    },
  };
}

export function InvitationTermsChoice({ terms, org, checked, onCheckedChange, disabled = false }: {
  terms: InvitationTerms | null | undefined; org: string; checked: boolean; onCheckedChange: (on: boolean) => void; disabled?: boolean;
}) {
  const { t } = useTranslation();
  if (terms === null) return null;
  if (terms === undefined) return <span role="alert" className="text-sm text-danger-text">{t('orgTerms.unavailable')}</span>;
  return <div className="flex min-w-0 flex-col gap-3">
    <div role="region" aria-label={t('orgTerms.title')} tabIndex={0} className="focus-inset max-h-64 min-w-0 overflow-auto rounded-lg border border-border bg-bg-subtle p-3">
      <Markdown text={terms.text} compact />
    </div>
    <Checkbox label={t('orgTerms.accept', { org })} checked={checked} disabled={disabled} onCheckedChange={onCheckedChange} />
  </div>;
}

export function MemberTerms({ member }: { member: Member }) {
  const { t } = useTranslation();
  const at: unknown = 'terms_accepted_at' in member ? member.terms_accepted_at : null;
  return typeof at === 'string' ? <span className="flex flex-wrap items-center gap-1 text-sm text-text-muted">{t('orgTerms.accepted')} · <RelativeTime at={at} /></span> : null;
}
