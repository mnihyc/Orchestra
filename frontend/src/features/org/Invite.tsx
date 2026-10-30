/** Invite…: one dialog that brings somebody into an org at any level, by a link for one address or a link to share, with the role, the time it is good for and the rules the org sets.
 *
 * What may be asked for is bounded by the org's own settings, which the dialog reads: the forms it
 * allows, how long an invitation is good by default and at most, and how many people a shared link
 * serves. Before anything is made the dialog says what whoever uses the link becomes; afterwards it
 * shows the link, once, to copy, and says what became of a personal link: sent, or to be handed on.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link2, Mail } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { ApiError } from '@/api/client';
import { metaQuery } from '@/api/auth';
import { targetSettingsQuery } from '@/api/settings';
import { createInvitation } from '@/api/users';
import { useIdentityRules } from '@/features/auth/identityRules';
import type { InvitationCreateReply, Org, ResolvedSetting, Role } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useActKey, useForm } from '@/lib/forms';
import { formatDateTime } from '@/lib/time';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';

/** The roles a person can be invited as, from the one that changes nothing to the one that runs the place. */
export const INVITED_ROLES: readonly Role[] = ['viewer', 'interactor', 'developer', 'admin'];

/** How long an invitation can be good for, offered in these steps up to what the org allows: a day, three, a week, two weeks, a month. */
const DURATION_STEPS_HOURS = [24, 72, 168, 336, 720] as const;

/** What an org's settings say of its invitations; what is missing from them is what the system ships. */
export interface InvitationRules {
  forms: readonly ('link' | 'email')[];
  defaultHours: number;
  maxHours: number;
  maxUses: number;
}

const SHIPPED: InvitationRules = { forms: ['link', 'email'], defaultHours: 168, maxHours: 720, maxUses: 100 };

export function invitationRules(settings: readonly ResolvedSetting[] | undefined): InvitationRules {
  const value = (key: string): unknown => settings?.find((setting) => setting.key === key)?.value;
  const number = (key: string, fallback: number): number => {
    const found = value(key);
    return typeof found === 'number' && found > 0 ? found : fallback;
  };
  const forms = value('invitations.forms');
  return {
    forms: Array.isArray(forms) ? forms.filter((form): form is 'link' | 'email' => form === 'link' || form === 'email') : SHIPPED.forms,
    defaultHours: number('invitations.default_hours', SHIPPED.defaultHours),
    maxHours: number('invitations.max_hours', SHIPPED.maxHours),
    maxUses: number('invitations.max_uses', SHIPPED.maxUses),
  };
}

/** The lengths of time offered: the steps up to the most, and the default and the most themselves. */
export function durationsFor(rules: Pick<InvitationRules, 'defaultHours' | 'maxHours'>): number[] {
  const steps = new Set<number>(DURATION_STEPS_HOURS.filter((hours) => hours <= rules.maxHours));
  steps.add(rules.defaultHours);
  steps.add(rules.maxHours);
  return [...steps].filter((hours) => hours <= rules.maxHours).sort((one, other) => one - other);
}

/** A no-mail system starts with a usable shared link, while the org's allowed forms still decide. */
export function initialInvitationKind(forms: readonly ('link' | 'email')[], mail: boolean): 'link' | 'email' {
  return forms.includes('email') && (mail || !forms.includes('link')) ? 'email' : 'link';
}

export interface InviteDialogProps {
  target: { kind: 'org'; org: Org } | { kind: 'system' };
  open: boolean;
  onClose: () => void;
}

export function InviteDialog({ target, open, onClose }: InviteDialogProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const identity = useIdentityRules();
  const actKey = useActKey();
  const org = target.kind === 'org' ? target.org : null;
  const settings = useQuery({ ...targetSettingsQuery(org === null ? { kind: 'system' } : { kind: 'node', id: org.id }), enabled: open, staleTime: Infinity });
  const meta = useQuery({ ...metaQuery, enabled: open });
  const rules = useMemo(() => invitationRules(settings.data), [settings.data]);
  const [kind, setKind] = useState<'email' | 'link' | null>(null);
  const [role, setRole] = useState<Role>('viewer');
  const [hours, setHours] = useState<number | null>(null);
  const [made, setMade] = useState<InvitationCreateReply | null>(null);

  // What is asked for begins as what the org gives by default, and is what the person changes from there.
  const chosenKind = kind !== null && rules.forms.includes(kind) ? kind : initialInvitationKind(rules.forms, meta.data?.mail === true);
  const ready = settings.isSuccess && meta.isSuccess && rules.forms.length > 0;
  const chosenHours = hours ?? rules.defaultHours;
  const form = useForm(
    { email: '', uses: '1' },
    {
      email: (value) => (chosenKind === 'email' ? identity.email(value) : null),
      uses: (value) => {
        if (chosenKind !== 'link') return null;
        const count = Number(value);
        return Number.isInteger(count) && count >= 1 && count <= rules.maxUses ? null : t('places.invite.usesRule', { max: rules.maxUses });
      },
    },
  );

  const make = useMutation({
    mutationFn: (values: { email: string; uses: string }) => {
      const body = {
        ...(org === null ? { kind: 'system' as const } : { kind: 'org' as const, org_id: org.id, role }),
        expires_in_hours: chosenHours,
        ...(chosenKind === 'email' ? { email: values.email.trim() } : { max_uses: Number(values.uses) }),
      };
      return createInvitation(body, actKey(body));
    },
    onSuccess: (reply) => {
      setMade(reply);
    },
    onError: (error) => void form.refuse(error),
  });

  const roleName = t(`roles.${role}`);
  const name = org?.name ?? meta.data?.name ?? t('access.place.system');
  const root = org?.path[0]?.name ?? name;
  const placed = make.error instanceof ApiError && make.error.faults().some((fault) => fault.path in form.values);
  const notAllowed = make.error instanceof ApiError && make.error.is('invitation.not_allowed');

  const reset = () => {
    setMade(null);
    form.set('email', '');
    make.reset();
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t('places.invite.title', { org: name })}
      dismissible={!make.isPending}
      footer={
        made === null ? (
          <>
            <Button disabled={make.isPending} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button type="submit" form="invite-form" variant="primary" pending={make.isPending} disabled={!ready}>
              {t('places.invite.submit')}
            </Button>
          </>
        ) : (
          <>
            <Button onClick={reset}>{t('places.invite.another')}</Button>
            <Button variant="primary" onClick={onClose}>
              {t('places.invite.done')}
            </Button>
          </>
        )
      }
    >
      {made === null && settings.isError ? <ErrorState compact error={settings.error} onRetry={() => void settings.refetch()} />
        : made === null && meta.isError ? <ErrorState compact error={meta.error} onRetry={() => void meta.refetch()} />
        : made === null && !ready ? rules.forms.length === 0 && settings.isSuccess ? <InlineAlert tone="info">{t('errors.invitation.not_allowed')}</InlineAlert> : <SkeletonRows rows={3} />
        : made === null ? (
        <form
          id="invite-form"
          noValidate
          className="flex flex-col gap-4 py-2"
          onSubmit={form.submit((values) => {
            if (!make.isPending && ready) make.mutate(values);
          })}
        >
          <SegmentedControl
            label={t('places.invite.form')}
            value={chosenKind}
            onValueChange={setKind}
            block
            segments={[
              { value: 'email', icon: <Mail aria-hidden />, ...choiceOption(t, 'invitationForm.email'), description: t('places.invite.forms.email'), disabled: !rules.forms.includes('email') },
              { value: 'link', icon: <Link2 aria-hidden />, ...choiceOption(t, 'invitationForm.link'), description: t('places.invite.forms.link'), disabled: !rules.forms.includes('link') },
            ]}
          />
          {rules.forms.length < 2 && <p className="-mt-2 text-sm text-text-muted">{t('places.invite.formLeftOut', { org: root })}</p>}
          {chosenKind === 'email' ? (
            <Input
              label={t('places.invite.email')}
              hint={t('places.invite.emailHint')}
              type="email"
              autoComplete="off"
              inputMode="email"
              {...form.field('email')}
            />
          ) : (
            <Input
              label={t('places.invite.uses')}
              hint={t(org === null ? 'places.invite.registerUsesHint' : 'places.invite.usesHint', { max: rules.maxUses })}
              type="number"
              min={1}
              max={rules.maxUses}
              inputMode="numeric"
              {...form.field('uses')}
            />
          )}
          <div className={org === null ? 'grid grid-cols-1 gap-4' : 'grid grid-cols-1 gap-4 sm:grid-cols-2'}>
            {org !== null && <Select
              label={t('places.invite.role')}
              value={role}
              onValueChange={setRole}
              options={INVITED_ROLES.map((value) => ({ value, ...choiceOption(t, `accessRole.${value}`) }))}
            />}
            <Select
              label={t('places.invite.goodFor')}
              value={String(chosenHours)}
              onValueChange={(value) => setHours(Number(value))}
              hint={t('places.invite.most', { max: durationLabel(rules.maxHours, t), org: root })}
              options={durationsFor(rules).map((value) => ({ value: String(value), ...choiceOption(t, 'dynamic.value', { name: durationLabel(value, t), context: t('places.invite.goodFor') }) }))}
            />
          </div>
          <p className="text-sm text-text-muted">
            {org === null ? t('places.invite.becomesSystem') : org.parent_id === null
              ? t('places.invite.becomesRoot', { role: roleName, org: org.name })
              : t('places.invite.becomes', { role: roleName, org: org.name, root })}
          </p>
          {org === null && meta.data?.registration_mode === 'closed' && <InlineAlert tone="warning">{t('places.invite.registrationClosed')}</InlineAlert>}
          {notAllowed && <InlineAlert tone="warning">{t('errors.invitation.not_allowed')}</InlineAlert>}
          {make.error !== null && !placed && !notAllowed && <ErrorAlert error={make.error} />}
        </form>
      ) : (
        <div className="flex flex-col gap-4 py-2">
          <InlineAlert tone={made.invitation.form === 'email' && !made.sent ? 'warning' : 'success'}>
            {made.invitation.form === 'link'
              ? t(org === null ? 'places.invite.madeRegistration' : 'places.invite.madeShared', {
                  count: made.invitation.max_uses,
                  time: formatDateTime(made.invitation.expires_at, format),
                })
              : made.sent
                ? t('places.invite.sent', { email: made.invitation.email ?? '' })
                : t('places.invite.notSent', { org: root, email: made.invitation.email ?? '' })}
          </InlineAlert>
          <div className="flex items-end gap-2">
            <Input
              label={t('places.invite.link')}
              hint={t('places.invite.linkOnce')}
              value={made.url}
              readOnly
              mono
              fieldClassName="flex-1"
              onFocus={(event) => event.currentTarget.select()}
            />
            <CopyButton value={made.url} label={t('places.invite.copy')} className="mb-6" />
          </div>
        </div>
      )}
    </Dialog>
  );
}

/** A length of time in words: whole days where it is whole days, hours otherwise. */
export function durationLabel(hours: number, t: (key: 'places.duration.days' | 'places.duration.hours', options: { count: number }) => string): string {
  return hours % 24 === 0 ? t('places.duration.days', { count: hours / 24 }) : t('places.duration.hours', { count: hours });
}
