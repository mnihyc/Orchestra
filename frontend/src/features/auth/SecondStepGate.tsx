/** What a page shows in place of an org's work that waits for the person's second step: Set up a second step, or Verify your second step, with the places that require it.
 *
 * An org may require a second step of its members. While the person has none set up, its work answers
 * that it waits for the setup, and this says so with the places that require it and a way to Security;
 * their own org and Security stay open. While they have one and this browser has not given it, this
 * asks for a code of it here, and once it is given the page loads what it was opening: the destination
 * is the page itself, which stays where it is. An identity provider's sign-in is not taken for a
 * second step.
 */
import { useMutation } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ShieldCheck } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { sendEmailCode, verifySecondStep } from '@/api/users';
import { recoveryCodeUsed } from '@/features/auth/recoveryCode';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { useErrorText } from '@/ui/errorText';
import { InlineAlert } from '@/ui/InlineAlert';
import { StateLine } from '@/ui/StateLine';
import { Input } from '@/ui/Input';

/** Whether `error` is a refusal that waits for the person's second step. */
export function waitsForSecondStep(error: unknown): error is ApiError {
  return error instanceof ApiError && (error.is('auth.second_step_setup') || error.is('auth.second_step_required'));
}

/** The names of the places that require the step, as the refusal names them; the system has a word of its own. */
function placesOf(error: ApiError, system: string): string {
  const scopes: unknown = error.problem.scopes;
  if (!Array.isArray(scopes)) return system;
  const names = scopes.map((scope: unknown) =>
    typeof scope === 'object' && scope !== null && 'name' in scope && typeof scope.name === 'string' ? scope.name : system,
  );
  return names.length === 0 ? system : names.join(', ');
}

export interface SecondStepGateProps {
  error: ApiError;
  /** Loads again what waited, once the step is given. */
  onDone?: (() => void) | undefined;
  className?: string;
}

export function SecondStepGate({ error, onDone, className }: SecondStepGateProps) {
  const { t } = useTranslation();
  const text = useErrorText();
  const places = placesOf(error, t('secondStep.system'));
  const setup = error.is('auth.second_step_setup');
  const methods = error.extra('auth.second_step_required')?.methods ?? [];
  const byApp = methods.includes('totp');
  const byMail = methods.includes('email_code');
  const byRecovery = methods.includes('recovery_code');
  const [way, setWay] = useState<'totp' | 'email_code' | 'recovery_code'>(byApp ? 'totp' : byRecovery ? 'recovery_code' : 'email_code');
  const [code, setCode] = useState('');
  const mail = useMutation({ mutationFn: () => sendEmailCode({}) });
  const verify = useMutation({
    mutationFn: () => verifySecondStep(way === 'totp' ? { totp: code.trim() } : way === 'recovery_code' ? { recovery_code: code.trim() } : { email_code: code.trim() }),
    onSuccess: () => {
      onDone?.();
      if (way === 'recovery_code') recoveryCodeUsed();
    },
  });
  return (
    <div className={cx('flex min-w-0 flex-col gap-3 py-3', className)}>
      <StateLine role="alert" icon={<ShieldCheck aria-hidden />} compact
        title={setup ? t('secondStep.setupTitle') : t('secondStep.verifyTitle')}
        message={setup ? t('secondStep.setupLead', { places }) : t('secondStep.verifyLead', { places })}
        action={setup ? <Button asChild size="sm"><Link to="/settings/security">{t('secondStep.setupAction')}</Link></Button> : undefined} />
      {!setup && (
        <form
          noValidate
          className="flex w-full max-w-(--container-form) flex-col gap-3 text-left"
          onSubmit={(event) => {
            event.preventDefault();
            if (code.trim() !== '' && !verify.isPending) verify.mutate();
          }}
        >
          <Input
            label={way === 'totp' ? t('secondStep.appCode') : way === 'recovery_code' ? t('auth.second.recoveryLabel') : t('secondStep.mailedCode')}
            inputMode={way === 'totp' ? 'numeric' : 'text'}
            autoComplete="one-time-code"
            mono
            disabled={verify.isPending || mail.isPending}
            value={code}
            onChange={(event) => setCode(event.target.value)}
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button type="submit" variant="primary" pending={verify.isPending} blocked={code.trim() === ''}>
              {t('secondStep.verifyAction')}
            </Button>
            {byApp && way !== 'totp' && <Button disabled={verify.isPending || mail.isPending} onClick={() => { setWay('totp'); setCode(''); verify.reset(); }}>{t('auth.second.use.totp')}</Button>}
            {byRecovery && way !== 'recovery_code' && <Button disabled={verify.isPending || mail.isPending} onClick={() => { setWay('recovery_code'); setCode(''); verify.reset(); }}>{t('auth.second.use.recovery_code')}</Button>}
            {byMail && (
              <Button
                pending={mail.isPending}
                disabled={verify.isPending}
                onClick={() => {
                  setWay('email_code');
                  setCode('');
                  verify.reset();
                  mail.mutate();
                }}
              >
                {byApp ? t('secondStep.mailInstead') : t('secondStep.mailMe')}
              </Button>
            )}
          </div>
          {mail.isSuccess && <InlineAlert tone="success">{t('secondStep.mailSent')}</InlineAlert>}
          {mail.error !== null && <InlineAlert tone="danger">{text(mail.error).message}</InlineAlert>}
          {verify.error !== null && <InlineAlert tone="danger">{text(verify.error).message}</InlineAlert>}
        </form>
      )}
    </div>
  );
}
