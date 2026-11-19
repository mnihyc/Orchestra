/** Password; the second step, by an authenticator app and, where the person's orgs allow it, by a code sent by mail; identities and sessions.
 *
 * The second step is a list of the ways there are, each with its switch and what it protects against.
 * Switching one on asks for the password and then for a code that proves it works (a code of the app
 * the person scanned, or one that was mailed); switching one off asks for the password and a code.
 * Sessions are the browsers that are signed in; the person ends any but the one they are using. A
 * password changed ends every other session, and the page says so before it is changed.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link2, Mail, Monitor, Smartphone } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { metaQuery, meQuery, PASSWORD_MIN_CHARS } from '@/api/auth';
import { ApiError } from '@/api/client';
import {
  changePassword,
  disableAuthenticator,
  disableEmailCode,
  enableAuthenticator,
  enableEmailCode,
  linkIdentity,
  revokeSession,
  revokeOtherSessions,
  sendEmailCode,
  sessionsQuery,
  type SetupProof,
  setUpAuthenticator,
  unlinkIdentity,
} from '@/api/users';
import { proveAgain, takeDraft, takeFreshProof, takeFreshProofError } from '@/features/auth/freshProof';
import { IdentityCallback } from '@/features/auth/IdentityCallback';
import { useIdentityRules } from '@/features/auth/identityRules';
import { recoveryCodesIssued } from '@/features/auth/recoveryCode';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { RECOVERY_DRAFT, ReplaceRecoveryCodes, SavedRecoveryCodes } from '@/features/settings/personal/RecoveryCodes';
import type { Me, MeIdentity, SessionInfo } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useForm } from '@/lib/forms';
import { formatDateTime } from '@/lib/time';
import { boxClasses, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';
import { QrCode } from '@/ui/QrCode';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { toast } from '@/ui/Toast';

/** What a browser calls itself, in the order the names are looked for: several carry the names of the ones they are built on. */
const BROWSERS: readonly (readonly [string, string])[] = [
  ['Edg/', 'Edge'],
  ['OPR/', 'Opera'],
  ['Firefox/', 'Firefox'],
  ['Chrome/', 'Chrome'],
  ['Safari/', 'Safari'],
];

const SYSTEMS: readonly (readonly [string, string])[] = [
  ['Windows', 'Windows'],
  ['Android', 'Android'],
  ['iPhone', 'iOS'],
  ['iPad', 'iOS'],
  ['Mac OS X', 'macOS'],
  ['Linux', 'Linux'],
];

/** A browser as a person would name it: "Chrome (Linux)", from the line it sends; the line itself where it is none we know. */
export function browserName(userAgent: string): string {
  const browser = BROWSERS.find(([needle]) => userAgent.includes(needle))?.[1];
  const system = SYSTEMS.find(([needle]) => userAgent.includes(needle))?.[1];
  if (browser === undefined) return userAgent;
  return system === undefined ? browser : `${browser} (${system})`;
}

// ---------------------------------------------------------------- the password

function PasswordForm({ me }: { me: Me }) {
  const { t } = useTranslation();
  const identity = useIdentityRules();
  const form = useForm(
    { current: '', next: '', again: '' },
    {
      current: (value) => (value === '' ? t('security.password.currentMissing') : null),
      next: (value, values) => identity.password(value, [me.username, me.email ?? '']) ?? (value === values.current ? t('security.password.sameAsOld') : null),
      again: (value, values) => (value !== values.next ? t('security.password.mismatch') : null),
    },
  );
  const save = useMutation({
    mutationFn: (values: { current: string; next: string }) => changePassword(values.current, values.next),
    onSuccess: () => {
      form.set('current', '');
      form.set('next', '');
      form.set('again', '');
      toast({ tone: 'success', title: t('security.password.changed'), description: t('security.password.changedAbout') });
    },
  });
  if (!me.has_password) return <p className="text-sm text-text-muted">{t('security.password.none')}</p>;
  return (
    <form
      noValidate
      className="flex max-w-(--container-form) flex-col gap-4"
      onSubmit={form.submit((values) => {
        if (!save.isPending) save.mutate({ current: values.current, next: values.next });
      })}
    >
      <PasswordInput label={t('security.password.current')} autoComplete="current-password" {...form.field('current')} />
      <PasswordInput label={t('security.password.new')} hint={t('security.password.newHint', { min: PASSWORD_MIN_CHARS })} autoComplete="new-password" {...form.field('next')} />
      <PasswordInput label={t('security.password.again')} autoComplete="new-password" {...form.field('again')} />
      <p className="text-sm text-text-muted">{t('security.password.endsOthers')}</p>
      {save.error !== null && <ErrorAlert error={save.error} />}
      <div>
        <Button type="submit" variant="primary" pending={save.isPending}>
          {t('security.password.change')}
        </Button>
      </div>
    </form>
  );
}

// ---------------------------------------------------------------- the second step

type Way = 'authenticator' | 'mail';

interface SecondStepDialogProps {
  way: Way;
  turning: 'on' | 'off';
  me: Me;
  onClose: () => void;
  /** The one-use proof the person's identity provider came back with, for a person who has no password. */
  identityProof?: string | null;
  /** The identity provider came back without a proof. */
  proofFailed?: boolean;
  /** The setup reply is the only time these codes can be saved. */
  onRecoveryCodes: (codes: string[]) => void;
}

/** The draft a setup keeps while a person without a password proves their sign-in again. */
const SETUP_DRAFT = 'second-step';

/** Switches a way of the second step on or off: the password first (or, without one, a fresh proof of the linked sign-in), and a code that shows it works. */
function SecondStepDialog({ way, turning, me, onClose, identityProof = null, proofFailed = false, onRecoveryCodes }: SecondStepDialogProps) {
  const { t } = useTranslation();
  // Switching on goes in two steps: the password, then the code the way produced.
  const [step, setStep] = useState<'password' | 'code'>(turning === 'on' ? 'password' : 'code');
  const [secret, setSecret] = useState<{ secret: string; url: string } | null>(null);
  // The code is asked for after the password, so it has its own state: its field must not be found wrong before it has been shown.
  const [code, setCode] = useState('');
  const [codeSent, setCodeSent] = useState(false);
  const asksPassword = me.has_password;
  // Without a password the setup is proved by the linked sign-in, proved again for this act.
  const linked = me.identities[0];
  const form = useForm(
    { password: '' },
    { password: (value) => (asksPassword && value === '' ? t('security.second.passwordMissing') : null) },
  );
  const codeWrong = step === 'code' && code.trim() === '' ? t('security.second.codeMissing') : undefined;

  const finished = (title: string) => {
    toast({ tone: 'success', title });
    onClose();
  };
  const begin = useMutation({
    mutationFn: async (password: string) => {
      const proof: SetupProof = asksPassword || identityProof === null ? { password } : { identity_proof: identityProof };
      if (way === 'authenticator') {
        const reply = await setUpAuthenticator(proof);
        setSecret({ secret: reply.secret, url: reply.otpauth_url });
      } else {
        await sendEmailCode(proof);
      }
    },
    onSuccess: () => setStep('code'),
    onError: (error) => void form.refuse(error),
  });
  const mail = useMutation({ mutationFn: (password: string) => sendEmailCode(asksPassword ? { password } : {}) });
  const finish = useMutation({
    mutationFn: async (values: { password: string }) => {
      const typed = code.trim();
      if (turning === 'on') {
        if (way !== 'authenticator') return enableEmailCode(typed);
        const reply = await enableAuthenticator(typed);
        onRecoveryCodes(reply.recovery_codes.codes);
        return reply.me;
      }
      return way === 'authenticator' ? disableAuthenticator(values.password, typed) : disableEmailCode(values.password, typed);
    },
    onSuccess: () =>
      finished(
        turning === 'on'
          ? way === 'authenticator'
            ? t('security.second.authenticatorOn')
            : t('security.second.mailOn')
          : way === 'authenticator'
            ? t('security.second.authenticatorOff')
            : t('security.second.mailOff'),
      ),
    onError: (error) => void form.refuse(error),
  });

  const busy = begin.isPending || finish.isPending || mail.isPending;
  const error = finish.error ?? begin.error;
  const placed = error instanceof ApiError && error.faults().some((fault) => fault.path in form.values);
  const toMail = turning === 'off' && way === 'mail';
  const title =
    way === 'authenticator'
      ? turning === 'on'
        ? t('security.second.authenticatorOnTitle')
        : t('security.second.authenticatorOffTitle')
      : turning === 'on'
        ? t('security.second.mailOnTitle')
        : t('security.second.mailOffTitle');
  const lead =
    turning === 'on' && step === 'password'
      ? t('security.second.passwordLead')
      : turning === 'on' && way === 'authenticator'
        ? t('security.second.scanLead')
        : turning === 'on'
          ? t('security.second.mailedLead', { email: me.email ?? '' })
          : t('security.second.offLead');

  const proving = step === 'password' && turning === 'on' && !asksPassword && identityProof === null;
  const prove = () => {
    if (linked !== undefined) proveAgain(linked.idp_key, 'second_step_setup', SETUP_DRAFT, { way });
  };
  const submit = form.submit((values) => {
    if (busy) return;
    if (proving) {
      prove();
      return;
    }
    if (step === 'password' && turning === 'on') {
      begin.mutate(values.password);
      return;
    }
    setCodeSent(true);
    if (codeWrong === undefined) finish.mutate(values);
  });

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !busy && onClose()}
      title={title}
      description={lead}
      dismissible={!busy}
      footer={
        <>
          <Button disabled={busy} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button type="submit" form="second-step" variant={turning === 'off' ? 'danger' : 'primary'} pending={begin.isPending || finish.isPending}>
            {step === 'password' && turning === 'on'
              ? t('common.continue')
              : turning === 'on'
                ? t('security.second.switchOn')
                : t('security.second.switchOff')}
          </Button>
        </>
      }
    >
      <form id="second-step" noValidate className="flex flex-col gap-4 py-2" onSubmit={submit}>
        {(step === 'password' || turning === 'off') && asksPassword && (
          <PasswordInput label={t('security.second.password')} autoComplete="current-password" {...form.field('password')} />
        )}
        {step === 'password' && turning === 'on' && !asksPassword && (
          identityProof === null ? (
            <div className="flex flex-col gap-2">
              {proofFailed && <InlineAlert tone="warning">{t('tokens.new.proveFailed')}</InlineAlert>}
              <p className="text-sm text-text-muted">
                {linked === undefined ? t('security.second.proveNone') : t('security.second.proveLead', { name: linked.idp_name })}
              </p>
              <div>
                <Button size="sm" blocked={linked === undefined} onClick={prove}>
                  {t('security.second.prove')}
                </Button>
              </div>
            </div>
          ) : (
            <InlineAlert tone="success">{t('security.second.proved')}</InlineAlert>
          )
        )}
        {turning === 'on' && step === 'code' && way === 'authenticator' && secret !== null && (
          <div className="flex flex-wrap items-start gap-4">
            <QrCode value={secret.url} label={t('security.second.qr')} />
            <div className="flex min-w-0 flex-1 basis-[min(100%,12rem)] flex-col gap-2">
              <p className="text-sm text-text-muted">{t('security.second.typeIt')}</p>
              <div className="flex items-center gap-2">
                <code className="rounded-md border border-border bg-bg-subtle px-2 py-1 font-mono text-sm wrap-anywhere text-text">{secret.secret}</code>
                <CopyButton value={secret.secret} label={t('security.second.copySecret')} />
              </div>
            </div>
          </div>
        )}
        {toMail && (
          <div className="flex flex-col gap-2">
            <p className="text-sm text-text-muted">{me.totp_enabled ? t('security.second.offCodeEither') : t('security.second.offCodeMailed')}</p>
            <div>
              <Button size="sm" pending={mail.isPending} onClick={() => mail.mutate(form.values.password)} blocked={asksPassword && form.values.password === ''}>
                {t('security.second.mailMe')}
              </Button>
            </div>
            {mail.isSuccess && <InlineAlert tone="success">{t('security.second.mailSent', { email: me.email ?? '' })}</InlineAlert>}
            {mail.error !== null && <ErrorAlert error={mail.error} />}
          </div>
        )}
        {step === 'code' && (
          <Input
            label={way === 'authenticator' && !toMail ? turning === 'off' ? t('security.recovery.disableCode') : t('security.second.appCode') : t('security.second.code')}
            inputMode={way === 'authenticator' && turning === 'on' ? 'numeric' : 'text'}
            autoComplete="one-time-code"
            mono
            value={code}
            error={codeSent ? codeWrong : undefined}
            onChange={(event) => setCode(event.target.value)}
          />
        )}
        {error !== null && !placed && <ErrorAlert error={error} />}
      </form>
    </Dialog>
  );
}

function WayRow({
  icon,
  title,
  on,
  about,
  onToggle,
  disabled,
  reason,
}: {
  icon: ReactNode;
  title: string;
  on: boolean;
  about: ReactNode;
  onToggle: (next: boolean) => void;
  disabled?: boolean;
  reason?: string;
}) {
  return (
    <li className="flex items-start gap-3 px-4 py-3">
      <span className="mt-0.5 text-text-subtle">{icon}</span>
      <div className="min-w-0 flex-1">
        <Switch presentation="compact" label={title} hint={reason ?? about} checked={on} disabled={disabled === true} onCheckedChange={onToggle} />
      </div>
    </li>
  );
}

/** The places that require a second step, as the person reads them. */
function requiredBy(me: Me, system: string): string {
  return me.second_step_sources.map((source) => (typeof source === 'string' ? system : (source.name ?? system))).join(', ');
}

function SecondStep({ me }: { me: Me }) {
  const { t } = useTranslation();
  // Back from proving the linked sign-in again: the proof is taken from the address, and the setup it was for goes on.
  const [returned] = useState(() => {
    const proof = takeFreshProof();
    const failed = proof === null && takeFreshProofError() !== null;
    if (proof === null && !failed) return null;
    const recovery = takeDraft(RECOVERY_DRAFT);
    const draft = takeDraft(SETUP_DRAFT) as { way: Way } | null;
    return { proof, failed, recovery: recovery !== null, way: draft?.way ?? 'authenticator' };
  });
  const [dialog, setDialog] = useState<{ way: Way; turning: 'on' | 'off' } | null>(returned === null || returned.recovery ? null : { way: returned.way, turning: 'on' });
  const [replacing, setReplacing] = useState(returned?.recovery === true);
  const [savedCodes, setSavedCodes] = useState<string[] | null>(null);
  const showCodes = (codes: string[]) => { recoveryCodesIssued(); setSavedCodes(codes); };
  const confirmed = me.email !== null && me.email_verified;
  const mailAllowed = me.email_code.includes('second_step');
  // A mailed code is offered where the orgs allow it and there is an address to send it to; one that is on can always be switched off.
  const showMail = me.email_code_second_step || (mailAllowed && confirmed);
  const setUp = me.totp_enabled || me.email_code_second_step;
  const places = requiredBy(me, t('secondStep.system'));
  return (
    <div className="flex flex-col gap-3">
      {me.second_step_required &&
        (setUp ? (
          <InlineAlert tone="info">{t('security.second.requiredSet', { places })}</InlineAlert>
        ) : (
          <InlineAlert tone="warning" title={t('security.second.requiredTitle')}>
            {t('security.second.required', { places })}
          </InlineAlert>
        ))}
      <ul className={`${boxClasses} divide-y divide-border`}>
        <WayRow
          icon={<Smartphone className="size-4" aria-hidden />}
          title={t('security.second.authenticator')}
          on={me.totp_enabled}
          about={t('security.second.authenticatorAbout')}
          onToggle={(next) => setDialog({ way: 'authenticator', turning: next ? 'on' : 'off' })}
        />
        {showMail && (
          <WayRow
            icon={<Mail className="size-4" aria-hidden />}
            title={t('security.second.mail')}
            on={me.email_code_second_step}
            about={t('security.second.mailAbout', { email: me.email ?? '' })}
            onToggle={(next) => setDialog({ way: 'mail', turning: next ? 'on' : 'off' })}
          />
        )}
      </ul>
      {me.totp_enabled && <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border px-4 py-3">
        <span className="text-base text-text">{t('security.recovery.title')}</span>
        <Button size="sm" onClick={() => setReplacing(true)}>{t('security.recovery.replace')}</Button>
      </div>}
      {mailAllowed && !confirmed && !me.email_code_second_step && <p className="text-sm text-text-muted">{t('security.second.mailNeedsAddress')}</p>}
      <p className="text-sm text-text-muted">{t('security.second.lost')}</p>
      {dialog !== null && (
        <SecondStepDialog
          way={dialog.way}
          turning={dialog.turning}
          me={me}
          identityProof={dialog.turning === 'on' && returned?.way === dialog.way ? returned.proof : null}
          proofFailed={dialog.turning === 'on' && returned?.way === dialog.way && returned.proof === null}
          onClose={() => setDialog(null)}
          onRecoveryCodes={showCodes}
        />
      )}
      {replacing && <ReplaceRecoveryCodes me={me} returned={returned?.recovery === true ? returned : null}
        onClose={() => setReplacing(false)} onIssued={(codes) => { setReplacing(false); showCodes(codes); }} />}
      {savedCodes !== null && dialog === null && <SavedRecoveryCodes codes={savedCodes} onClose={() => setSavedCodes(null)} />}
    </div>
  );
}

// ---------------------------------------------------------------- linked identities

/** The member of the fragment this page is sent back with when an identity was not linked. */
const LINK_ERROR_FRAGMENT = 'identity_link_error';

/** Why the identity provider's sign-in was not linked, taken from the address and cleared from it; `null` when the page was not sent back with one. */
function takeLinkError(): string | null {
  const hash = window.location.hash;
  const fragment = new URLSearchParams(hash.startsWith('#') ? hash.slice(1) : hash);
  const why = fragment.get(LINK_ERROR_FRAGMENT);
  if (why === null) return null;
  window.history.replaceState(window.history.state, '', `${window.location.pathname}${window.location.search}`);
  return why;
}

function Identities({ me }: { me: Me }) {
  const { t } = useTranslation();
  const meta = useQuery(metaQuery);
  const format = useFormatContext();
  const [removing, setRemoving] = useState<MeIdentity | null>(null);
  // Back from the provider without a link: why, once.
  const [linkError] = useState(takeLinkError);
  const providers = meta.data?.identity_providers ?? [];
  const unlinked = providers.filter((provider) => !me.identities.some((identity) => identity.idp_key === provider.key));
  const unlink = useMutation({
    mutationFn: (identity: MeIdentity) => unlinkIdentity(identity.idp_key),
    onSuccess: () => {
      setRemoving(null);
    },
  });
  const link = useMutation({
    mutationFn: (key: string) => linkIdentity(key),
    onSuccess: (address) => window.location.assign(address),
  });
  // Unlinking the last way in is refused: that is said in words of its own.
  const onlyWay = unlink.error instanceof ApiError && unlink.error.is('resource.in_use');
  if (me.identities.length === 0 && unlinked.length === 0 && linkError === null) return null;
  return (
    <Section id="security-identities" title={t('security.identities.title')} lead={t('security.identities.lead')}>
      {linkError !== null &&
        (linkError === 'denied' ? (
          <InlineAlert tone="danger">{t('security.identities.linkDenied')}</InlineAlert>
        ) : (
          <IdentityCallback code={linkError} />
        ))}
      <ul className={`${boxClasses} divide-y divide-border`}>
        {me.identities.map((identity) => (
          <li key={identity.idp_key} className="flex flex-wrap items-center gap-3 px-4 py-3">
            <Link2 className="size-4 text-text-subtle" aria-hidden />
            <div className="min-w-0 flex-1">
              <p className="text-base font-medium text-text">{identity.idp_name}</p>
              <p className="text-sm text-text-muted">
                {identity.email === null ? t('security.identities.since', { time: formatDateTime(identity.linked_at, format) }) : t('security.identities.as', { email: identity.email, time: formatDateTime(identity.linked_at, format) })}
              </p>
            </div>
            <Button size="sm" variant="ghost" onClick={() => setRemoving(identity)}>
              {t('security.identities.unlink')}
            </Button>
          </li>
        ))}
        {unlinked.map((provider) => (
          <li key={provider.key} className="flex flex-wrap items-center gap-3 px-4 py-3">
            <Link2 className="size-4 text-text-subtle" aria-hidden />
            <p className="min-w-0 flex-1 text-base text-text">{provider.name}</p>
            <Button size="sm" pending={link.isPending && link.variables === provider.key} onClick={() => link.mutate(provider.key)}>
              {t('security.identities.link', { name: provider.name })}
            </Button>
          </li>
        ))}
      </ul>
      {link.error !== null && <ErrorAlert error={link.error} />}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(next) => !next && setRemoving(null)}
        title={t('security.identities.unlinkTitle', { name: removing?.idp_name ?? '' })}
        confirmLabel={t('security.identities.unlinkConfirm', { name: removing?.idp_name ?? '' })}
        danger
        pending={unlink.isPending}
        error={onlyWay ? undefined : (unlink.error ?? undefined)}
        onConfirm={() => removing !== null && unlink.mutate(removing)}
      >
        {t('security.identities.unlinkBody')}
        {onlyWay && (
          <InlineAlert tone="danger" className="mt-3">
            {t('security.identities.onlyWay')}
          </InlineAlert>
        )}
      </ConfirmDialog>
    </Section>
  );
}

// ---------------------------------------------------------------- sessions

function Sessions() {
  const { t } = useTranslation();
  const format = useFormatContext();
  const list = useQuery(sessionsQuery);
  const [ending, setEnding] = useState<SessionInfo | null>(null);
  const [endingOthers, setEndingOthers] = useState(false);
  const others = useMutation({
    mutationFn: revokeOtherSessions,
    onSuccess: () => {
      setEndingOthers(false);
      toast({ tone: 'success', title: t('security.sessions.othersEnded') });
    },
  });
  const end = useMutation({
    mutationFn: (session: SessionInfo) => revokeSession(session.id),
    onSuccess: (_none, session) => {
      setEnding(null);
      toast({ tone: 'success', title: t('security.sessions.ended', { browser: browserName(session.user_agent) }) });
    },
  });
  return (
    <Section id="security-sessions" title={t('security.sessions.title')} lead={t('security.sessions.lead')}
      action={list.data?.some((session) => !session.current) === true ? <Button size="sm" disabled={end.isPending || others.isPending} onClick={() => setEndingOthers(true)}>{t('security.sessions.endOthers')}</Button> : undefined}>
      {list.isPending ? (
        <SkeletonRows rows={2} />
      ) : list.isError ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : (
        <ul className={`${boxClasses} divide-y divide-border`}>
          {[...list.data].sort((one, other) => Number(other.current) - Number(one.current) || other.last_used_at.localeCompare(one.last_used_at)).map((session) => (
            <li key={session.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
              <Monitor className="size-4 text-text-subtle" aria-hidden />
              <div className="min-w-0 flex-1">
                <p className="flex flex-wrap items-center gap-2 text-base font-medium text-text">
                  <span className="wrap-anywhere">{browserName(session.user_agent) || t('security.sessions.unknown')}</span>
                  {session.current && <Badge tone="accent">{t('security.sessions.thisOne')}</Badge>}
                </p>
                <p className="flex flex-wrap gap-x-3 text-sm text-text-muted">
                  {session.ip !== null ? <span className="font-mono">{session.ip}</span> : <span>{t('security.sessions.notReported')}</span>}
                  <span>
                    {t('security.sessions.lastUsed')} <RelativeTime at={session.last_used_at} />
                  </span>
                  <span>{t('security.sessions.ends', { time: formatDateTime(session.expires_at, format) })}</span>
                </p>
              </div>
              {!session.current && (
                <Button size="sm" variant="ghost" onClick={() => setEnding(session)}>
                  {t('security.sessions.end')}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      <ConfirmDialog
        open={endingOthers}
        onOpenChange={(open) => !open && setEndingOthers(false)}
        title={t('security.sessions.endOthersTitle')}
        confirmLabel={t('security.sessions.endOthersConfirm')}
        danger pending={others.isPending} error={others.error ?? undefined}
        onConfirm={() => others.mutate()}
      >{t('security.sessions.endOthersBody')}</ConfirmDialog>
      <ConfirmDialog
        open={ending !== null}
        onOpenChange={(next) => !next && setEnding(null)}
        title={t('security.sessions.endTitle', { browser: browserName(ending?.user_agent ?? '') })}
        confirmLabel={t('security.sessions.endConfirm')}
        danger
        pending={end.isPending}
        error={end.error ?? undefined}
        onConfirm={() => ending !== null && end.mutate(ending)}
      >
        {t('security.sessions.endBody')}
      </ConfirmDialog>
    </Section>
  );
}

export function Security() {
  const { t } = useTranslation();
  const me = useQuery(meQuery);
  return (
    <PersonalFrame page="security" lead={t('security.lead')}>
      {me.isPending ? (
        <SkeletonRows rows={4} />
      ) : me.isError ? (
        <ErrorState error={me.error} retrying={me.isRefetching} onRetry={() => void me.refetch()} />
      ) : (
        <div className="flex flex-col gap-6">
          <Section id="security-password" title={t('security.password.title')}>
            <div className={`${boxClasses} p-4`}>
              <PasswordForm me={me.data} />
            </div>
          </Section>
          <Section id="security-second" title={t('security.second.title')} lead={t('security.second.lead')}>
            <SecondStep me={me.data} />
          </Section>
          <Identities me={me.data} />
          <Sessions />
        </div>
      )}
    </PersonalFrame>
  );
}
