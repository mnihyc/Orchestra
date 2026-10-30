/** Sign-in: identity providers, a password, a code sent by mail where somebody may use one, the second step as the answer names it, and a refusal that says which proof is closed and until when.
 *
 * A sign-in takes one proof, or two different ones from somebody who has a second step. The server
 * keeps nothing between two requests: each one carries every proof given so far. So this screen
 * remembers, for as long as it is open and nowhere else, the proof that was right, and asks for the
 * one the answer says is missing.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link, useRouter } from '@tanstack/react-router';
import { type ReactNode, type SyntheticEvent, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { askEmailCode, metaQuery, resendConfirmation, signIn } from '@/api/auth';
import { ApiError, type SignInProof } from '@/api/client';
import { EmailCode } from '@/features/auth/EmailCode';
import { IdentityProviders } from '@/features/auth/IdentityProviders';
import { AuthCard } from '@/features/auth/Frame';
import { IdentityCallback } from '@/features/auth/IdentityCallback';
import { recoveryCodeUsed } from '@/features/auth/recoveryCode';
import type { AuthLoginBody, Meta } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useForm } from '@/lib/forms';
import { formatTimeOfDay } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';
import { linkButtonClasses, linkClasses } from '@/ui/link';

/** The proofs that were right so far, kept in this screen's memory alone. */
interface Proofs {
  login: string;
  password?: string;
  email_code?: string;
  totp?: string;
  recovery_code?: string;
}

type Stage =
  /** A name and a password. */
  | { at: 'first' }
  /** A code that was mailed as the first proof. */
  | { at: 'code'; login: string; sentAt: number }
  /** One proof more, of the ones the answer named. */
  | { at: 'second'; proofs: Proofs; methods: SignInProof[]; using: SignInProof; sentAt: number | null };

export interface LoginProps {
  /** Where to go on to once signed in: a path inside the application. */
  onward: string | null;
  /** The code a sign-in through an identity provider came back with, when it was refused. */
  callbackError: string | null;
}

/** Which of the proofs that would complete a sign-in is asked for first: the one that needs no mail. */
function firstOf(methods: SignInProof[]): SignInProof | null {
  return (['totp', 'recovery_code', 'email_code', 'password'] as const).find((method) => methods.includes(method)) ?? null;
}

export function Login({ onward, callbackError }: LoginProps) {
  const { t } = useTranslation();
  const router = useRouter();
  const meta = useQuery(metaQuery).data;
  const [stage, setStage] = useState<Stage>({ at: 'first' });
  /** What the last attempt was refused with, when the refusal belongs to the stage in view. */
  const [refused, setRefused] = useState<unknown>(null);
  /** How many attempts were refused: each one puts the reader back into the field to mend. */
  const [failures, setFailures] = useState(0);
  const [adminForm, setAdminForm] = useState(false);

  const enter = useMutation({ mutationFn: (body: AuthLoginBody) => signIn(body) });
  const code = useMutation({ mutationFn: askEmailCode });

  const done = () => {
    if (onward === null) void router.navigate({ to: '/', replace: true });
    else router.history.replace(onward);
  };

  /** Tries to sign in with what is at hand, and moves to the second step when the answer asks for one. */
  const attempt = (proofs: Proofs, more: Partial<Record<SignInProof, string>> = {}, onRefused?: () => void) => {
    if (enter.isPending) return;
    setRefused(null);
    enter.mutate(
      { ...proofs, ...more },
      {
        onSuccess: () => {
          done();
          if (more.recovery_code !== undefined || proofs.recovery_code !== undefined) recoveryCodeUsed();
        },
        onError: (error) => {
          const methods = error instanceof ApiError ? error.extra('auth.second_step_required')?.methods.filter((method) => method !== 'email_code' || meta?.mail === true) : undefined;
          const using = methods === undefined ? null : firstOf(methods);
          if (methods === undefined || using === null) {
            setRefused(error);
            setFailures((count) => count + 1);
            onRefused?.();
            return;
          }
          const right: Proofs = { ...proofs, ...more };
          // A code that is the only second step is on its way before the field for it is shown.
          if (using === 'email_code' && right.password !== undefined) {
            code.mutate({ login: right.login, password: right.password });
            setStage({ at: 'second', proofs: right, methods, using, sentAt: Date.now() });
          } else {
            setStage({ at: 'second', proofs: right, methods, using, sentAt: null });
          }
        },
      },
    );
  };

  const restart = () => {
    setRefused(null);
    enter.reset();
    setStage({ at: 'first' });
  };

  if (meta === undefined) return null;

  const title =
    stage.at === 'second' ? t('auth.second.title') : stage.at === 'code' ? t('auth.code.title') : t('auth.login.title');

  return (
    <AuthCard
      title={title}
      foot={
        stage.at === 'first' ? (
          <FirstFoot meta={meta} />
        ) : (
          <button type="button" className={linkButtonClasses} onClick={restart}>
            {t('auth.login.startOver')}
          </button>
        )
      }
    >
      {stage.at === 'first' && (
        <FirstStep
          meta={meta}
          onward={onward}
          callbackError={callbackError}
          adminForm={adminForm}
          onAdminForm={() => setAdminForm(true)}
          pending={enter.isPending}
          asking={code.isPending}
          refused={refused}
          onSubmit={(login, password, onRefused) => attempt({ login, password }, {}, onRefused)}
          onMailCode={(login) => {
            setRefused(null);
            code.mutate({ login });
            setStage({ at: 'code', login, sentAt: Date.now() });
          }}
        />
      )}

      {stage.at === 'code' && (
        <EmailCode
          key={stage.sentAt}
          sentence={t('auth.code.sent')}
          sentAt={stage.sentAt}
          pending={enter.isPending}
          resending={code.isPending}
          canResend={meta.mail}
          failures={failures}
          fieldError={isWrongProof(refused) ? t('auth.code.wrong') : undefined}
          refusal={<Refusal error={isWrongProof(refused) ? null : refused} meta={meta} way="email_code" />}
          onSubmit={(typed) => attempt({ login: stage.login }, { email_code: typed })}
          onResend={() => {
            setRefused(null);
            code.mutate({ login: stage.login });
            setStage({ ...stage, sentAt: Date.now() });
          }}
        >
          <button type="button" className={linkButtonClasses} onClick={restart}>
            {t('auth.code.usePassword')}
          </button>
        </EmailCode>
      )}

      {stage.at === 'second' && (
        <SecondStep
          stage={stage}
          meta={meta}
          pending={enter.isPending}
          asking={code.isPending}
          refused={refused}
          failures={failures}
          onSubmit={(value) => attempt(stage.proofs, { [stage.using]: value })}
          onUse={(using) => {
            setRefused(null);
            if (using === 'email_code' && stage.proofs.password !== undefined) {
              code.mutate({ login: stage.proofs.login, password: stage.proofs.password });
              setStage({ ...stage, using, sentAt: Date.now() });
            } else {
              setStage({ ...stage, using });
            }
          }}
        />
      )}
    </AuthCard>
  );
}

/** Whether the refusal says that the proof just given was wrong, which is said at its field. */
function isWrongProof(error: unknown): boolean {
  return error instanceof ApiError && (error.is('auth.invalid_credentials') || error.is('auth.second_step_invalid'));
}

// ---------------------------------------------------------------- the first step

interface FirstStepProps {
  meta: Meta;
  onward: string | null;
  callbackError: string | null;
  adminForm: boolean;
  onAdminForm: () => void;
  pending: boolean;
  asking: boolean;
  refused: unknown;
  /** Signs in. `onRefused` is called when the attempt was refused, to empty the password for a new one. */
  onSubmit: (login: string, password: string, onRefused: () => void) => void;
  onMailCode: (login: string) => void;
}

function FirstStep({
  meta,
  onward,
  callbackError,
  adminForm,
  onAdminForm,
  pending,
  asking,
  refused,
  onSubmit,
  onMailCode,
}: FirstStepProps) {
  const { t } = useTranslation();
  const form = useForm(
    { login: '', password: '' },
    {
      login: (value) => (value.trim() === '' ? t('auth.rules.loginMissing') : null),
      password: (value) => (value === '' ? t('auth.rules.passwordMissing') : null),
    },
  );
  const [nameAsked, setNameAsked] = useState(false);
  const wrong = isWrongProof(refused);

  const providers = meta.identity_providers;
  const showForm = meta.local_login || adminForm;
  const login = form.field('login');
  const password = form.field('password');

  return (
    <div className="flex flex-col gap-4">
      {callbackError !== null && <IdentityCallback code={callbackError} />}

      {showForm && (
        <form
          noValidate
          className="flex flex-col gap-4"
          onSubmit={form.submit((values) => {
            if (pending) return;
            // A password that was refused is typed again; the name stays.
            onSubmit(values.login.trim(), values.password, () => {
              form.set('password', '');
              form.focus('password');
            });
          })}
        >
          {!meta.local_login && <InlineAlert tone="info">{t('auth.login.adminOnly')}</InlineAlert>}
          <Input
            label={t('auth.fields.login')}
            autoComplete="username"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            autoFocus
            {...login}
            error={login.error ?? (nameAsked && form.values.login.trim() === '' ? t('auth.rules.loginFirst') : undefined)}
          />
          <PasswordInput
            label={t('auth.fields.password')}
            autoComplete="current-password"
            {...password}
            // A password that was refused was emptied to be typed again: that it was wrong is what there is to say.
            error={wrong && password.value === '' ? t('auth.login.wrong') : (password.error ?? (wrong ? t('auth.login.wrong') : undefined))}
          />
          {meta.mail && <Link to="/recover" className={`${linkClasses} self-end text-sm`}>
            {t('auth.links.forgot')}
          </Link>}
          <Refusal error={wrong ? null : refused} meta={meta} way="password" login={form.values.login.trim()} />
          <Button type="submit" variant="primary" block pending={pending}>
            {t('auth.login.submit')}
          </Button>
        </form>
      )}

      {providers.length > 0 && showForm && (
        <div className="flex items-center gap-3 text-sm text-text-subtle" role="separator">
          <span className="h-px flex-1 bg-border" />
          {t('auth.login.or')}
          <span className="h-px flex-1 bg-border" />
        </div>
      )}

      <IdentityProviders providers={providers} redirect={onward} />

      {showForm && meta.mail && meta.email_code_login && (
        <button
          type="button"
          className={`${linkButtonClasses} self-start text-sm`}
          disabled={asking}
          onClick={() => {
            const name = form.values.login.trim();
            if (name === '') {
              setNameAsked(true);
              form.focus('login');
              return;
            }
            onMailCode(name);
          }}
        >
          {t('auth.login.mailCode')}
        </button>
      )}

      {!showForm && (
        <div className="flex flex-col items-start gap-2">
          {providers.length === 0 && <InlineAlert tone="info">{t('auth.login.noWay')}</InlineAlert>}
          <button type="button" className={`${linkButtonClasses} text-sm`} onClick={onAdminForm}>
            {t('auth.login.adminSignIn')}
          </button>
        </div>
      )}
    </div>
  );
}


function FirstFoot({ meta }: { meta: Meta }) {
  const { t } = useTranslation();
  if (meta.registration_mode === 'closed') return <p>{t('auth.login.closed')}</p>;
  if (meta.registration_mode === 'invite') return <p>{t('auth.login.byInvitation')}</p>;
  return (
    <p>
      {t('auth.login.noAccount')}{' '}
      <Link to="/register" className={linkClasses}>
        {t('auth.links.register')}
      </Link>
    </p>
  );
}

// ---------------------------------------------------------------- the second step

interface SecondStepProps {
  stage: Extract<Stage, { at: 'second' }>;
  meta: Meta;
  pending: boolean;
  asking: boolean;
  refused: unknown;
  failures: number;
  onSubmit: (value: string) => void;
  onUse: (method: SignInProof) => void;
}

function SecondStep({ stage, meta, pending, asking, refused, failures, onSubmit, onUse }: SecondStepProps) {
  const { t } = useTranslation();
  const wrong = isWrongProof(refused);
  const refusal = <Refusal error={wrong ? null : refused} meta={meta} way={stage.using} />;
  const others = stage.methods.filter((method) => method !== stage.using && (method !== 'email_code' || meta.mail));
  const account = <p className="text-sm wrap-anywhere text-text-muted">{t('auth.second.account', { account: stage.proofs.login })}</p>;

  const switches = others.map((method) => (
    <button key={method} type="button" className={linkButtonClasses} disabled={asking || pending} onClick={() => onUse(method)}>
      {t(`auth.second.use.${method}`)}
    </button>
  ));

  if (stage.using === 'email_code') {
    return (
      <>{account}<EmailCode
        key={stage.sentAt ?? 0}
        sentence={t('auth.second.codeSent')}
        sentAt={stage.sentAt ?? 0}
        pending={pending}
        resending={asking}
        canResend={meta.mail}
        failures={failures}
        fieldError={wrong ? t('auth.code.wrong') : undefined}
        refusal={refusal}
        onSubmit={onSubmit}
        onResend={() => onUse('email_code')}
      >
        {switches}
      </EmailCode></>
    );
  }

  return (
    <>{account}<SecretStep
      key={stage.using}
      kind={stage.using}
      pending={pending}
      wrong={wrong}
      failures={failures}
      refusal={refusal}
      onSubmit={onSubmit}
    >
      {switches}
    </SecretStep></>
  );
}

interface SecretStepProps {
  kind: 'totp' | 'password' | 'recovery_code';
  pending: boolean;
  wrong: boolean;
  /** How many attempts were refused so far; each one selects what was typed, to be typed again. */
  failures: number;
  refusal: ReactNode;
  onSubmit: (value: string) => void;
  children?: ReactNode;
}

/** The second step that is typed from something the person has or knows: the authenticator's code, or the password. */
function SecretStep({ kind, pending, wrong, failures, refusal, onSubmit, children }: SecretStepProps) {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  const [missing, setMissing] = useState(false);
  const field = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (failures > 0) field.current?.select();
  }, [failures]);

  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    if (pending) return;
    const typed = kind === 'totp' ? value.replace(/\s+/g, '') : kind === 'recovery_code' ? value.trim() : value;
    if (typed === '') {
      setMissing(true);
      field.current?.focus();
      return;
    }
    onSubmit(typed);
  };

  const empty = kind === 'password' ? value === '' : value.trim() === '';
  const error = missing && empty ? t(`auth.second.missing.${kind}`) : wrong ? t(`auth.second.wrong.${kind}`) : undefined;
  const change = (text: string) => {
    setValue(text);
    setMissing(false);
  };
  const instructions = kind === 'recovery_code' ? t('auth.second.lead.recovery_code') : null;

  return (
    <form noValidate className="flex flex-col gap-4" onSubmit={submit}>
      {instructions !== null && <p className="text-base text-text-muted">{instructions}</p>}
      {kind !== 'password' ? (
        <Input
          ref={field}
          label={kind === 'totp' ? t('auth.second.totpLabel') : t('auth.second.recoveryLabel')}
          mono
          autoFocus
          inputMode={kind === 'totp' ? 'numeric' : 'text'}
          autoComplete="one-time-code"
          value={value}
          error={error}
          onChange={(event) => change(event.target.value)}
        />
      ) : (
        <PasswordInput
          ref={field}
          label={t('auth.fields.password')}
          autoFocus
          autoComplete="current-password"
          value={value}
          error={error}
          onChange={(event) => change(event.target.value)}
        />
      )}
      {refusal}
      <Button type="submit" variant="primary" block pending={pending}>
        {t('auth.login.submit')}
      </Button>
      {children !== undefined && <div className="flex flex-col items-start gap-2 text-sm">{children}</div>}
    </form>
  );
}

// ---------------------------------------------------------------- refusals

interface RefusalProps {
  error: unknown;
  meta: Meta;
  /** The proof that was being given. */
  way: SignInProof;
  /** The name as typed, for what a refusal offers to do about it. */
  login?: string;
}

/** What a sign-in was refused with, where that is more than "wrong": which proof is closed and until when, and what else works. */
function Refusal({ error, meta, way, login }: RefusalProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const again = useMutation({ mutationFn: resendConfirmation });

  if (error === null || error === undefined) return null;
  if (!(error instanceof ApiError)) return <ErrorAlert error={error} />;

  if (error.is('auth.locked')) {
    const closed = error.extra('auth.locked')?.way ?? way;
    const message =
      error.retryAt === null
        ? t('auth.locked.noTime')
        : closed === 'password' && !meta.mail
          ? t('auth.locked.passwordNoMail', { time: formatTimeOfDay(error.retryAt, { locale, timeZone }) })
          : t(`auth.locked.${closed}`, { time: formatTimeOfDay(error.retryAt, { locale, timeZone }) });
    return (
      <ErrorAlert
        error={error}
        message={message}
        action={
          closed === 'password' && meta.mail ? (
            <Link to="/recover" className={linkClasses}>
              {t('auth.links.reset')}
            </Link>
          ) : undefined
        }
      />
    );
  }

  if (error.is('auth.email_unverified') && login !== undefined && login !== '') {
    return (
      <ErrorAlert
        error={error}
        message={!meta.mail ? t('auth.verify.noMail') : again.isSuccess ? t('auth.verify.resent') : undefined}
        action={
          !meta.mail || again.isSuccess ? undefined : (
            <button
              type="button"
              className={linkButtonClasses}
              disabled={again.isPending}
              onClick={() => again.mutate(login)}
            >
              {t('auth.verify.resend')}
            </button>
          )
        }
      />
    );
  }

  return <ErrorAlert error={error} />;
}
