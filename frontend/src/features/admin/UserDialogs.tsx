/** The acts on a person that ask first: making one, the link by which they set a new password, suspending and resuming them, and making them a system admin or not.
 *
 * A personal org is created only when enabled, with optional administrator overrides; the answer is the link by which they set their password. A change to a
 * system admin, or making somebody one, is acknowledged by whoever makes it, in the dialog and then in
 * the request. What is done to a person is shown as under way until the server has answered.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { createUser, makeResetLink, systemSettingsQuery, updateUser } from '@/api/admin';
import { USERNAME_PATTERN } from '@/api/auth';
import type { AdminUser, AdminUserCreateReply, AdminUserResetLinkCreateReply } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useActKey, useForm } from '@/lib/forms';
import { formatDateTime } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { toast } from '@/ui/Toast';
import { UserChip } from '@/ui/UserChip';

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** A link to hand on, shown whole in a field that can be selected, with the button that copies it. */
export function LinkField({ url, label, copyLabel }: { url: string; label: string; copyLabel: string }) {
  return (
    <Input
      label={label}
      value={url}
      readOnly
      mono
      onFocus={(event) => event.currentTarget.select()}
      trailing={<CopyButton value={url} label={copyLabel} />}
    />
  );
}

function CreatedUser({ reply, onClose }: { reply: AdminUserCreateReply; onClose: () => void }) {
  const { t } = useTranslation();
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={t('admin.users.made.title', { name: reply.user.display_name })}
      description={t('admin.users.made.lead')}
      footer={
        <Button variant="primary" onClick={onClose}>
          {t('admin.common.done')}
        </Button>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <UserChip user={reply.user} size={32} showUsername />
        <LinkField url={reply.reset_url} label={t('admin.users.link.url')} copyLabel={t('admin.users.link.copy')} />
        <InlineAlert tone="info">{t('admin.users.made.note')}</InlineAlert>
      </div>
    </Dialog>
  );
}

export function CreateUserDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const policy = useQuery(systemSettingsQuery);
  const setting = (key: string) => policy.data?.find((item) => item.key === key)?.value !== false;
  const personalEnabled = policy.isSuccess && setting('orgs.personal_enabled');
  const keyOf = useActKey();
  const form = useForm(
    { username: '', display_name: '', email: '' },
    {
      username: (value) => (USERNAME_PATTERN.test(value) ? null : t('admin.users.create.usernameRule')),
      display_name: (value) => (value.trim() === '' ? t('admin.users.create.nameRule') : null),
      email: (value) => (value.trim() === '' || EMAIL.test(value.trim()) ? null : t('admin.users.create.emailRule')),
    },
  );
  // Only what the person changed from the system's policy is theirs; what they did not touch follows the policy as it is read.
  const [alone, setAlone] = useState<boolean | null>(null);
  const [flat, setFlat] = useState<boolean | null>(null);
  const [admin, setAdmin] = useState(false);
  const [made, setMade] = useState<AdminUserCreateReply | null>(null);
  /** Whether the server's faults were placed at the fields they name. */
  const [placed, setPlaced] = useState(false);

  const closed = alone ?? setting('orgs.personal_closed');
  const noOrgs = flat ?? setting('orgs.personal_flat');

  const create = useMutation({
    mutationFn: (values: { username: string; display_name: string; email: string }) => {
      const body = {
        username: values.username,
        display_name: values.display_name.trim(),
        ...(values.email.trim() === '' ? {} : { email: values.email.trim() }),
        is_system_admin: admin,
        ...(personalEnabled && (alone !== null || flat !== null) ? { personal_org: {
          ...(alone === null ? {} : { closed: alone }),
          ...(flat === null ? {} : { flat }),
        } } : {}),
      };
      return createUser(body, keyOf(body));
    },
    onSuccess: (reply) => {
      setMade(reply);
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });

  if (made !== null) return <CreatedUser reply={made} onClose={onClose} />;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !create.isPending && onClose()}
      title={t('admin.users.create.title')}
      description={t('admin.users.create.lead')}
      dismissible={!create.isPending}
      footer={
        <>
          <Button disabled={create.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="admin-create-user" pending={create.isPending} disabled={!policy.isSuccess}>
            {t('admin.users.create.submit')}
          </Button>
        </>
      }
    >
      <form id="admin-create-user" className="flex flex-col gap-4 py-2" onSubmit={form.submit((values) => { if (policy.isSuccess && !create.isPending) create.mutate(values); })} noValidate>
        <Input label={t('admin.users.create.username')} hint={t('admin.users.create.usernameHint')} autoComplete="off" autoCapitalize="none" {...form.field('username')} />
        <Input label={t('admin.users.create.name')} autoComplete="off" {...form.field('display_name')} />
        <Input label={t('admin.users.create.email')} optional type="email" autoComplete="off" hint={t('admin.users.create.emailHint')} {...form.field('email')} />
        {personalEnabled && <fieldset className="flex flex-col gap-2 rounded-lg border border-border p-3">
          <legend className="px-1 text-sm font-medium text-text">{t('admin.users.create.ownOrg')}</legend>
          <Checkbox label={t('admin.users.create.alone')} checked={closed} onCheckedChange={setAlone} />
          <Checkbox label={t('admin.users.create.noOrgs')} checked={noOrgs} onCheckedChange={setFlat} />
          <p className="text-sm text-text-muted">{t('admin.users.create.ownOrgHint')}</p>
        </fieldset>}
        {policy.isSuccess && !personalEnabled && <InlineAlert tone="info">{t('admin.users.create.noOwnOrg')}</InlineAlert>}
        {policy.isPending && <p role="status">{t('common.loading')}</p>}
        {policy.isError && <ErrorState compact error={policy.error} retrying={policy.isRefetching} onRetry={() => void policy.refetch()} />}
        <Checkbox label={t('admin.users.create.admin')} hint={t('admin.users.adminSentence')} checked={admin} onCheckedChange={setAdmin} />
        {create.error !== null && !placed && <ErrorAlert error={create.error} />}
      </form>
    </Dialog>
  );
}

export function ResetLinkDialog({ user, onClose }: { user: AdminUser; onClose: () => void }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const [clear, setClear] = useState(false);
  const [link, setLink] = useState<AdminUserResetLinkCreateReply | null>(null);
  const hasSecondStep = user.totp_enabled || user.email_code_second_step;

  const make = useMutation({
    mutationFn: () => makeResetLink(user.id, { clear_second_step: clear }),
    onSuccess: setLink,
  });

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t('admin.users.link.title', { name: user.display_name })}
      description={t('admin.users.link.lead')}
      dismissible={!make.isPending}
      footer={
        link === null ? (
          <>
            <Button disabled={make.isPending} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" pending={make.isPending} onClick={() => make.mutate()}>
              {t('admin.users.link.make')}
            </Button>
          </>
        ) : (
          <Button variant="primary" onClick={onClose}>
            {t('admin.common.done')}
          </Button>
        )
      }
    >
      <div className="flex flex-col gap-4 py-2">
        {link === null ? (
          <>
            <Checkbox
              label={t('admin.users.link.clear')}
              hint={hasSecondStep ? t('admin.users.link.clearHint') : t('admin.users.link.noSecondStep')}
              checked={clear}
              disabled={!hasSecondStep}
              onCheckedChange={setClear}
            />
            {make.error !== null && <ErrorAlert error={make.error} />}
          </>
        ) : (
          <>
            <LinkField url={link.url} label={t('admin.users.link.url')} copyLabel={t('admin.users.link.copy')} />
            <p className="text-sm text-text-muted">{t('admin.users.link.expires', { time: formatDateTime(link.expires_at, { locale, timeZone }) })}</p>
            {clear && <InlineAlert tone="info">{t('admin.users.link.clearDone')}</InlineAlert>}
          </>
        )}
      </div>
    </Dialog>
  );
}

/** What is changed in a person's standing. */
export type Standing = 'suspend' | 'resume' | 'grant' | 'revoke';

/** The act asks once, says what it does, and for a system admin asks for the risk to be acknowledged before it can be confirmed. */
export function StandingDialog({ user, act, onClose }: { user: AdminUser; act: Standing; onClose: () => void }) {
  const { t } = useTranslation();
  const [acknowledged, setAcknowledged] = useState(false);
  // Touching a system admin, or making somebody one, is an act whose risk is acknowledged.
  const needsAcknowledgement = user.is_system_admin || act === 'grant';
  const danger = act === 'suspend' || act === 'revoke';

  const change = useMutation({
    mutationFn: () =>
      updateUser(user.id, {
        ...(act === 'suspend' ? { status: 'suspended' as const } : act === 'resume' ? { status: 'active' as const } : { is_system_admin: act === 'grant' }),
        ...(needsAcknowledgement ? { acknowledge_admin_risk: true } : {}),
      }),
    onSuccess: () => {
      toast({ tone: 'success', title: t(`admin.users.${act}.done`, { name: user.display_name }) });
      onClose();
    },
  });

  return (
    <Dialog
      open
      size="sm"
      onOpenChange={(open) => !open && !change.isPending && onClose()}
      title={t(`admin.users.${act}.title`, { name: user.display_name })}
      dismissible={!change.isPending}
      footer={
        <>
          <Button disabled={change.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant={danger ? 'danger' : 'primary'}
            pending={change.isPending}
            disabled={needsAcknowledgement && !acknowledged}
            onClick={() => change.mutate()}
          >
            {t(`admin.users.${act}.confirm`, { name: user.display_name })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <p className="text-base text-text-muted">{t(`admin.users.${act}.body`)}</p>
        {needsAcknowledgement && (
          <Checkbox
            label={t(act === 'grant' ? 'admin.users.acknowledgeGrant' : 'admin.users.acknowledge')}
            checked={acknowledged}
            onCheckedChange={setAcknowledged}
            disabled={change.isPending}
          />
        )}
        {change.error !== null && <ErrorAlert error={change.error} />}
      </div>
    </Dialog>
  );
}
