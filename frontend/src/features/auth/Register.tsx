/** Registration as the system allows it: by an invitation, openly, or not at all; a personal link asking for the address it was sent to.
 *
 * What this screen shows depends on what the system allows and on what the visitor holds. With an
 * invitation it first says what the invitation is. Without one it shows the form where anybody may
 * register, and otherwise says in one sentence how people get in here.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link, useRouter } from '@tanstack/react-router';
import { CircleCheck, DoorClosed, MailCheck, Ticket } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { metaQuery, nameAvailabilityQuery, PASSWORD_MIN_CHARS, register, resendConfirmation, USERNAME_PATTERN } from '@/api/auth';
import { ApiError, invitationIssuerAuthorityLost } from '@/api/client';
import { invitationQuery } from '@/api/users';
import { AuthCard } from '@/features/auth/Frame';
import { IdentityProviders } from '@/features/auth/IdentityProviders';
import { acceptedDestination } from '@/features/auth/accepted';
import { useIdentityRules } from '@/features/auth/identityRules';
import { InvitationSkeleton, InvitationSpent, InvitationSummary, isSpent } from '@/features/auth/Invitation';
import type { InvitationAcceptReply, InvitationPreview } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';
import { linkButtonClasses, linkClasses } from '@/ui/link';

/** How a registration ended, as this screen goes on from it. */
type Ended =
  /** Signed in; the accepted reply alone supplies the permitted destination. */
  | { as: 'in'; accepted: InvitationAcceptReply | null; invited: boolean; personalOrg: boolean }
  /** The address must be confirmed first: a link went to it. */
  | { as: 'confirm'; login: string; email: string };

export function Register({ invitation }: { invitation: string | null }) {
  const { t } = useTranslation();
  const router = useRouter();
  const meta = useQuery(metaQuery).data;
  const preview = useQuery({ ...invitationQuery(invitation ?? ''), enabled: invitation !== null });
  const [ended, setEnded] = useState<Ended | null>(null);
  const invitationOnly = meta?.registration_mode === 'invite' && invitation === null;
  useEffect(() => { if (invitationOnly) void router.navigate({ to: '/login', replace: true }); }, [invitationOnly, router]);

  const signIn = (
    <p>
      {t('auth.register.haveAccount')}{' '}
      <Link to="/login" search={{ redirect: invitation === null ? undefined : `/invite/${encodeURIComponent(invitation)}` }} className={linkClasses}>
        {t('auth.links.signIn')}
      </Link>
    </p>
  );

  if (meta === undefined || invitationOnly) return null;
  if (ended?.as === 'in') return <Welcome ended={ended} />;
  if (ended?.as === 'confirm') return <ConfirmFirst login={ended.login} email={ended.email} mail={meta.mail} />;

  const title = t('auth.register.title');

  if (invitation !== null) {
    if (preview.isPending) {
      return (
        <AuthCard title={title} foot={signIn}>
          <InvitationSkeleton />
        </AuthCard>
      );
    }
    if (preview.isError && !isSpent(preview.error)) {
      return (
        <AuthCard title={title} foot={signIn}>
          <ErrorState compact error={preview.error} retrying={preview.isRefetching} onRetry={() => void preview.refetch()} />
        </AuthCard>
      );
    }
  }

  const good = invitation !== null && preview.isSuccess ? preview.data : null;
  const spent = invitation !== null && preview.isError;

  // Without an invitation that is still good, the system's own rule decides.
  if (meta.registration_mode === 'closed' || (good === null && meta.registration_mode !== 'open')) {
    return (
      <AuthCard title={title} foot={signIn}>
        {spent && <InvitationSpent />}
        <EmptyState
          compact
          icon={meta.registration_mode === 'closed' ? <DoorClosed aria-hidden /> : <Ticket aria-hidden />}
          title={meta.registration_mode === 'closed' ? t('auth.register.closed') : t('auth.register.byInvitation')}
        />
      </AuthCard>
    );
  }

  return (
    <AuthCard title={title} foot={signIn}>
      {spent && <InvitationSpent />}
      {good !== null && <InvitationSummary preview={good} system={meta.name} />}
      <RegisterForm invitation={good === null ? null : invitation} preview={good} onEnded={setEnded} />
      <IdentityProviders providers={meta.identity_providers} redirect="/" registration
        invitation={good === null ? null : invitation} acceptInvitation={good !== null && good.kind !== 'system'} />
    </AuthCard>
  );
}

interface RegisterFormProps {
  /** The secret of an invitation that is still good, when the visitor holds one. */
  invitation: string | null;
  preview: InvitationPreview | null;
  onEnded: (ended: Ended) => void;
}

function RegisterForm({ invitation, preview, onEnded }: RegisterFormProps) {
  const { t } = useTranslation();
  const rules = useIdentityRules();
  const bound = preview?.email_bound === true;
  const form = useForm(
    { username: '', display_name: '', email: '', password: '' },
    {
      username: rules.username,
      display_name: rules.displayName,
      // A personal link is for one address, and asks for it.
      email: bound ? rules.email : rules.optionalEmail,
      password: (value, values) => rules.password(value, [values.username, values.email]),
    },
  );
  const publicName = useDebounced(form.values.username);
  const checksName = publicName === form.values.username && USERNAME_PATTERN.test(publicName);
  const availability = useQuery({...nameAvailabilityQuery(publicName),enabled:checksName});
  const username = form.field('username');
  const nameStatus = !checksName ? null : availability.isFetching ? t('auth.register.nameChecking')
    : availability.isError ? t('auth.register.nameCheckFailed')
    : availability.data === undefined ? null : availability.data.available ? t('auth.register.nameAvailable') : t('auth.register.nameTaken');

  const create = useMutation({
    mutationFn: async (values: typeof form.values): Promise<Ended> => {
      const email = values.email.trim();
      const outcome = await register({
        username: values.username,
        display_name: values.display_name.trim(),
        password: values.password,
        ...(email === '' ? {} : { email }),
        ...(invitation === null ? {} : { invitation }),
        accept_invitation: preview !== null && preview.kind !== 'system',
      });
      if (!outcome.signedIn) return { as: 'confirm', login: values.username, email };
      return { as: 'in', accepted: outcome.accepted, invited: invitation !== null,
        personalOrg: outcome.result.user.personal_org_id !== null };
    },
    onSuccess: onEnded,
    onError: (error) => {
      if (form.refuse(error)) return;
      // The one refusal a personal link has of its own: another address than the one it was sent to.
      if (bound && error instanceof ApiError && error.is('registration.invite_required') && !invitationIssuerAuthorityLost(error)) form.focus('email');
    },
  });

  const error = create.error;
  const placed = error instanceof ApiError && error.faults().some((fault) => fault.path in form.values);
  const otherAddress = bound && error instanceof ApiError && error.is('registration.invite_required') && !invitationIssuerAuthorityLost(error);
  const taken = error instanceof ApiError && error.is('conflict.exists');
  const nameConflict = taken && error.faults().some((fault) => fault.path === 'username') && username.error !== undefined;
  const email = form.field('email');

  return (
    <form
      noValidate
      className="flex flex-col gap-4"
      onSubmit={form.submit((values) => {
        if (!create.isPending) create.mutate(values);
      })}
    >
      <Input
        label={t('auth.fields.username')}
        hint={<>{t('auth.fields.usernameHint')}{nameStatus !== null && <span className="mt-1 block" role="status">{nameStatus}</span>}</>}
        autoComplete="username"
        autoCapitalize="none"
        autoCorrect="off"
        spellCheck={false}
        autoFocus
        {...username}
        error={nameConflict ? t('auth.register.nameTaken') : username.error}
        onChange={(event) => form.set('username', event.target.value.toLowerCase())}
      />
      <Input
        label={t('auth.fields.displayName')}
        hint={t('auth.fields.displayNameHint')}
        autoComplete="name"
        {...form.field('display_name')}
      />
      <Input
        label={t('auth.fields.email')}
        optional={!bound}
        hint={bound ? t('auth.register.emailBound') : t('auth.register.emailHint')}
        type="email"
        inputMode="email"
        autoComplete="email"
        {...email}
        error={email.error ?? (otherAddress ? t('auth.register.otherAddress') : undefined)}
      />
      <PasswordInput
        label={t('auth.fields.password')}
        hint={t('auth.fields.passwordHint', { min: PASSWORD_MIN_CHARS })}
        autoComplete="new-password"
        {...form.field('password')}
      />
      {error !== null && !placed && !otherAddress && (
        <ErrorAlert error={error} message={taken ? t('auth.register.taken') : undefined} />
      )}
      <Button type="submit" variant="primary" block pending={create.isPending}>
        {t(preview?.kind === 'org' ? 'auth.register.submitJoin' : preview !== null && preview.kind !== 'system' ? 'auth.register.submitAccept' : 'auth.register.submit')}
      </Button>
    </form>
  );
}

/** The confirmed outcome, including a personal organization only when one was actually created. */
function Welcome({ ended }: { ended: Extract<Ended, { as: 'in' }> }) {
  const { t } = useTranslation();
  const router = useRouter();
  const org = ended.accepted?.org ?? null;
  const go = () => router.navigate({...acceptedDestination(ended.accepted ?? {org:null,node:null,chat:null}),replace:true});

  return (
    <AuthCard title={t('auth.register.welcomeTitle')}>
      <EmptyState
        compact
        icon={<CircleCheck aria-hidden />}
        title={
          org !== null
            ? t('auth.register.welcomeOrg', { org: org.name })
            : ended.invited
              ? t('auth.register.welcomeInvited')
              : t('auth.register.welcomeOwn')
        }
        description={ended.personalOrg ? t('auth.register.ownOrgToo') : undefined}
        action={
          <Button variant="primary" autoFocus onClick={() => void go()}>
            {t('common.continue')}
          </Button>
        }
      />
    </AuthCard>
  );
}

/** What somebody reads whose address has to be confirmed before they can sign in. */
function ConfirmFirst({ login, email, mail }: { login: string; email: string; mail: boolean }) {
  const { t } = useTranslation();
  const again = useMutation({ mutationFn: () => resendConfirmation(login) });
  return (
    <AuthCard
      title={t('auth.register.confirmTitle')}
      foot={
        <Link to="/login" className={linkClasses}>
          {t('auth.links.backToSignIn')}
        </Link>
      }
    >
      <EmptyState
        compact
        icon={<MailCheck aria-hidden />}
        title={mail ? t('auth.register.confirmSent', { email }) : t('auth.register.confirmNeeded')}
        description={!mail ? t('auth.verify.noMail') : t('auth.register.confirmThenIn')}
      />
      {!mail ? null : again.isSuccess ? (
        <InlineAlert tone="success">{t('auth.verify.resent')}</InlineAlert>
      ) : (
        <div className="flex flex-col items-center gap-2 text-sm">
          {again.error !== null && <ErrorAlert error={again.error} className="self-stretch" />}
          <button type="button" className={linkButtonClasses} disabled={again.isPending} onClick={() => again.mutate()}>
            {t('auth.verify.resend')}
          </button>
        </div>
      )}
    </AuthCard>
  );
}
