/** One-time recovery-code display and fresh-proof replacement; saved codes never enter a query or mutation cache. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { recoveryCodesQuery, regenerateRecoveryCodes, sendEmailCode } from '@/api/users';
import { ApiError } from '@/api/client';
import { proveAgain } from '@/features/auth/freshProof';
import type { Me, RecoveryCodeStatus, SensitiveProof } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';

/** The safe intent kept across an identity-provider round trip; no password or saved code is kept. */
export const RECOVERY_DRAFT = 'recovery-codes';

/** Plaintext exists only in the parent's ephemeral dialog state, cleared when this closes. */
export function SavedRecoveryCodes({ codes, onClose }: { codes: readonly string[]; onClose: () => void }) {
  const { t } = useTranslation();
  return <Dialog open onOpenChange={(open) => !open && onClose()} title={t('security.recovery.savedTitle')}
    description={t('security.recovery.saveOnce')}
    footer={<><CopyButton value={codes.join('\n')} label={t('security.recovery.copy')} /><Button variant="primary" onClick={onClose}>{t('security.recovery.done')}</Button></>}>
    <ul aria-label={t('security.recovery.list')} className="grid grid-cols-1 gap-2 py-2 sm:grid-cols-2">
      {codes.map((code) => <li key={code} className="select-all rounded-md border border-border bg-bg-subtle px-3 py-2 font-mono text-sm text-text">{code}</li>)}
    </ul>
  </Dialog>;
}

export interface ReturnedRecoveryProof { proof: string | null; failed: boolean }

/** Metadata is read only while this explicit management dialog is open. */
export function ReplaceRecoveryCodes({ me, returned, onClose, onIssued }: {
  me: Me;
  returned: ReturnedRecoveryProof | null;
  onClose: () => void;
  onIssued: (codes: string[]) => void;
}) {
  const { t } = useTranslation();
  const status = useQuery(recoveryCodesQuery);
  if (status.isSuccess && !status.data.methods.includes('totp')) return <Dialog open title={t('security.recovery.title')}
    onOpenChange={(open) => !open && onClose()}>
    <InlineAlert tone="info">{t('security.recovery.authenticatorOff')}</InlineAlert>
  </Dialog>;
  return status.isSuccess ? <ReplacementForm me={me} status={status.data} returned={returned} onClose={onClose} onIssued={onIssued} /> : (
    <Dialog open title={t('security.recovery.replaceTitle')} onOpenChange={(open) => !open && onClose()}>
      {status.isError ? <ErrorState compact error={status.error} retrying={status.isRefetching} onRetry={() => void status.refetch()} /> : <SkeletonRows rows={2} />}
    </Dialog>
  );
}

type CodeWay = 'totp' | 'email_code' | 'recovery_code';
const CODE_WAYS: readonly CodeWay[] = ['totp', 'recovery_code', 'email_code'];
const FORM = 'replace-recovery-codes';

function ReplacementForm({ me, status, returned, onClose, onIssued }: {
  me: Me; status: RecoveryCodeStatus; returned: ReturnedRecoveryProof | null;
  onClose: () => void; onIssued: (codes: string[]) => void;
}) {
  const { t } = useTranslation();
  const [password, setPassword] = useState('');
  const [code, setCode] = useState('');
  const ways = CODE_WAYS.filter((way) => status.methods.includes(way));
  const [way, setWay] = useState<CodeWay>(ways[0] ?? 'totp');
  const [submitted, setSubmitted] = useState(false);
  const provider = me.identities[0];
  const [identityProof, setIdentityProof] = useState(returned?.proof ?? null);
  const needsIdentity = !me.has_password && identityProof === null;
  const firstMissing = me.has_password ? password === '' : identityProof === null;
  const [needsCode, setNeedsCode] = useState(!status.recent_recovery);
  const codeMissing = needsCode && code.trim() === '';
  const mail = useMutation({ mutationFn: () => sendEmailCode({}) });
  const replace = useMutation({
    // Only safe metadata is retained by the mutation. The one-time plaintext goes straight to the dialog.
    mutationFn: async () => {
      const first: SensitiveProof = me.has_password ? { password } : { identity_proof: identityProof ?? '' };
      const proof: SensitiveProof = needsCode ? { ...first, [way]: code.trim() } : first;
      const reply = await regenerateRecoveryCodes(proof);
      setPassword('');
      setCode('');
      onIssued(reply.codes);
      return reply.status;
    },
    onError: (error) => {
      if (error instanceof ApiError && (error.is('auth.second_step_required') || !needsCode && error.is('auth.second_step_invalid'))) setNeedsCode(true);
      if (error instanceof ApiError && error.is('auth.reauthentication_required') && !me.has_password) setIdentityProof(null);
    },
  });
  const busy = replace.isPending || mail.isPending;
  return <Dialog open title={t('security.recovery.replaceTitle')} description={t('security.recovery.replaceAbout')}
    dismissible={!busy} onOpenChange={(open) => !open && !busy && onClose()}
    footer={<><Button disabled={busy} onClick={onClose}>{t('common.cancel')}</Button><Button type="submit" form={FORM} variant="primary" pending={replace.isPending} blocked={needsIdentity || ways.length === 0 && needsCode}>{t('security.recovery.generate')}</Button></>}>
    <form id={FORM} noValidate className="flex flex-col gap-3 py-2" onSubmit={(event) => {
      event.preventDefault();
      setSubmitted(true);
      if (!busy && !firstMissing && !codeMissing) replace.mutate();
    }}>
      <p className="text-sm text-text-muted">{t('security.recovery.remaining', { count: status.remaining })}</p>
      {me.has_password ? <PasswordInput autoComplete="current-password" label={t('security.second.password')} value={password} disabled={busy}
        error={submitted && password === '' ? t('security.second.passwordMissing') : undefined} onChange={(event) => setPassword(event.target.value)} /> : (
        identityProof === null ? <div className="flex flex-col items-start gap-2">
          {returned?.failed === true && <InlineAlert tone="warning">{t('tokens.new.proveFailed')}</InlineAlert>}
          <p className="text-sm text-text-muted">{provider === undefined ? t('security.second.proveNone') : t('security.recovery.proveLead', { name: provider.idp_name })}</p>
          <Button disabled={busy} blocked={provider === undefined} onClick={() => {
            if (provider !== undefined) proveAgain(provider.idp_key, 'recovery_codes', RECOVERY_DRAFT, { replace: true });
          }}>{t('security.second.prove')}</Button>
        </div> : <InlineAlert tone="success">{t('security.recovery.proved')}</InlineAlert>
      )}
      {needsCode ? <>
        <Input label={way === 'totp' ? t('auth.second.totpLabel') : way === 'recovery_code' ? t('auth.second.recoveryLabel') : t('secondStep.mailedCode')}
          autoComplete="one-time-code" inputMode={way === 'totp' ? 'numeric' : 'text'} mono value={code} disabled={busy}
          error={submitted && codeMissing ? t('security.second.codeMissing') : undefined} onChange={(event) => setCode(event.target.value)} />
        <div className="flex flex-wrap gap-2">{ways.filter((candidate) => candidate !== way).map((candidate) => <Button key={candidate} size="sm" disabled={busy} onClick={() => {
          setWay(candidate); setCode(''); setSubmitted(false); replace.reset();
          if (candidate === 'email_code') mail.mutate();
        }}>{t(`auth.second.use.${candidate}`)}</Button>)}
          {way === 'email_code' && <Button size="sm" pending={mail.isPending} onClick={() => mail.mutate()}>{t('secondStep.mailMe')}</Button>}
        </div>
        {mail.isSuccess && <InlineAlert tone="success">{t('secondStep.mailSent')}</InlineAlert>}
        {mail.error !== null && <ErrorAlert error={mail.error} />}
      </> : <p className="text-sm text-text-muted">{t('security.recovery.justVerified')}</p>}
      {replace.error !== null && <ErrorAlert error={replace.error} />}
    </form>
  </Dialog>;
}
