/** Name, avatar, address with its state and a change that waits for its confirmation, language, timezone, theme and density.
 *
 * Who the person is to others (a name with the picture it makes), the address they can be written to
 * and its state, and how the interface looks to them. Changing the address asks for the password and
 * changes nothing until the new address is confirmed; the old one stays the one that codes and
 * resets go to meanwhile. Language, theme and density are the person's own and easily undone, so
 * they show at once and are put back if the server refuses.
 */
import { useIsMutating, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Mail, Monitor, Moon, Sun } from 'lucide-react';
import { useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { metaQuery, meKey, meQuery, resendConfirmation, updateMe } from '@/api/auth';
import { ApiError } from '@/api/client';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { AutoSaveFeedback, useAutoSave } from '@/features/settings/AutoSave';
import { useIdentityRules } from '@/features/auth/identityRules';
import type { Me } from '@/generated/api-types';
import { LOCALES } from '@/i18n';
import { adoptAppearance, useAppearance } from '@/lib/appearance';
import { useForm } from '@/lib/forms';
import { avatarText, IDENTITY_COLORS } from '@/lib/grapheme';
import { boxClasses, Section } from '@/shell/Page';
import { Avatar } from '@/ui/Avatar';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';
import { Input, PasswordInput } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { SwatchGroup } from '@/ui/SwatchGroup';
import { TimeZonePicker } from '@/ui/TimeZonePicker';
import { toast } from '@/ui/Toast';

/** The me route's DISPLAY_NAME_CHARS upper bound, counted as Unicode scalars. */
const DISPLAY_NAME_MAX_CHARS = 64;
const preferenceWriteKey = (id: string) => ['personal-preferences', id] as const;

/** Changes the address: the password is asked, and nothing changes until the new address is confirmed. */
function ChangeAddress({ me, open, onClose }: { me: Me; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const identity = useIdentityRules();
  const meta = useQuery(metaQuery);
  const asksPassword = me.has_password;
  const form = useForm(
    { email: '', password: '' },
    {
      email: (value) => identity.email(value) ?? (value.trim().toLowerCase() === (me.email ?? '').toLowerCase() ? t('profile.address.same') : null),
      password: (value) => (asksPassword && value === '' ? t('profile.address.passwordRequired') : null),
    },
  );
  const save = useMutation({
    mutationFn: (values: { email: string; password: string }) =>
      updateMe({ email: values.email.trim(), ...(asksPassword ? { current_password: values.password } : {}) }),
    onSuccess: (updated) => {
      toast({
        tone: 'success',
        title: updated.email_pending === null ? t('profile.address.saved') : t('profile.address.sentTo', { email: updated.email_pending }),
      });
      onClose();
    },
    onError: (error) => void form.refuse(error),
  });
  // What the server found wrong with a field is said at that field; the rest is said for the form as a whole.
  const placed = save.error instanceof ApiError && save.error.faults().some((fault) => fault.path in form.values);
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !save.isPending && onClose()}
      title={t('profile.address.changeTitle')}
      description={t('profile.address.changeLead')}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button type="submit" form="change-address" variant="primary" pending={save.isPending}>
            {t('profile.address.change')}
          </Button>
        </>
      }
    >
      <form
        id="change-address"
        noValidate
        className="flex flex-col gap-4 py-2"
        onSubmit={form.submit((values) => {
          if (!save.isPending) save.mutate(values);
        })}
      >
        <Input
          label={t('profile.address.new')}
          type="email"
          inputMode="email"
          autoComplete="email"
          hint={meta.data?.mail === false ? t('profile.address.noMail') : t('profile.address.hint')}
          {...form.field('email')}
        />
        {asksPassword && (
          <PasswordInput label={t('profile.address.password')} autoComplete="current-password" {...form.field('password')} />
        )}
        {save.error !== null && !placed && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}

/** Clearing the address is explicit and proves the current password when the person has one. */
function ClearAddress({ me, onClose }: { me: Me; onClose: () => void }) {
  const {t} = useTranslation();
  const form = useForm({current_password:''},{current_password:(value) => me.has_password && value === '' ? t('profile.address.passwordRequired') : null});
  const clear = useMutation({
    mutationFn: (values: typeof form.values) => updateMe({email:null,...(me.has_password ? {current_password:values.current_password} : {})}),
    onSuccess: () => { toast({tone:'success',title:t('profile.address.cleared')}); onClose(); },
    onError: (error) => void form.refuse(error),
  });
  const placed = clear.error instanceof ApiError && clear.error.faults().some((fault) => fault.path in form.values);
  return <Dialog open onOpenChange={(open) => !open && !clear.isPending && onClose()} title={t('profile.address.clearTitle')} description={t('profile.address.clearLead')} dismissible={!clear.isPending} footer={<><Button disabled={clear.isPending} onClick={onClose}>{t('common.cancel')}</Button><Button form="clear-address" type="submit" variant="primary" pending={clear.isPending}>{t('profile.address.clear')}</Button></>}>
    <form id="clear-address" noValidate className="flex flex-col gap-4 py-2" onSubmit={form.submit((values) => {if (!clear.isPending) clear.mutate(values);})}>
      {me.has_password && <PasswordInput label={t('profile.address.password')} autoComplete="current-password" {...form.field('current_password')} />}
      {clear.error !== null && !placed && <ErrorAlert error={clear.error} />}
    </form>
  </Dialog>;
}

/** The shared twelve-colour palette writes its index; resetting asks the server for the person's original colour. */
function AvatarColour({me}: {me:Me}) {
  const {t} = useTranslation();
  const queryClient = useQueryClient();
  const busy = useIsMutating({ mutationKey: preferenceWriteKey(me.id), exact: true }) > 0;
  const save = useAutoSave<number | null>({ current: { value: me.avatar.color }, blocked: busy, mutationKey: preferenceWriteKey(me.id), write: async (color) => {
    const accepted = await updateMe({ avatar_color: color }); queryClient.setQueryData(meKey, accepted); return { value: accepted.avatar.color };
  } });
  return <div className="flex flex-col gap-3 border-t border-border pt-4">
    <SwatchGroup
      label={t('profile.avatar.label')}
      value={String(save.value ?? me.avatar.color)}
      onValueChange={(value) => save.choose(Number(value))}
      disabled={save.pending}
      options={Array.from({ length: IDENTITY_COLORS }, (_, color) => ({
        value: String(color),
        name: t('profile.avatar.color', { number: color + 1 }),
        sample: <Avatar text={me.avatar.text} color={color} size={24} />,
      }))}
    />
    <div><Button size="sm" variant="ghost" pending={save.pending} onClick={() => save.choose(null)}>{t('profile.avatar.reset')}</Button></div>
    <AutoSaveFeedback save={save} name={t('profile.avatar.label')} />
  </div>;
}

/** The address with its state, and what can be done about it. */
function AddressRow({ me }: { me: Me }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const [changing, setChanging] = useState(false);
  const [clearing, setClearing] = useState(false);
  const meta = useQuery(metaQuery);
  const passwordWorks = me.has_password && (meta.data?.local_login === true || me.is_system_admin);
  const identityWorks = me.identities.some((identity) => meta.data?.identity_providers.some((provider) => provider.key === identity.idp_key) === true);
  const clearReason = me.email_code_second_step ? t('profile.address.clearSecondStep') : meta.isPending ? t('profile.address.clearChecking')
    : !passwordWorks && !identityWorks ? t('profile.address.clearLastWay') : null;
  const resend = useMutation({
    mutationFn: () => resendConfirmation(me.username),
    onSuccess: () => toast({ tone: 'success', title: t('profile.address.resent', { email: me.email ?? '' }) }),
    onError: (error) => toast({ tone: 'danger', title: t('profile.address.notResent'), description: describe(error).message }),
  });
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Mail className="size-4 text-text-subtle" aria-hidden />
        {me.email === null ? (
          <span className="text-base text-text-muted">{t('profile.address.none')}</span>
        ) : (
          <>
            <span data-user-content className="text-base font-medium wrap-anywhere text-text">{me.email}</span>
            {me.email_verified ? <Badge tone="neutral">{t('profile.address.confirmed')}</Badge> : <Badge tone="warning">{t('profile.address.unconfirmed')}</Badge>}
          </>
        )}
      </div>
      {me.email !== null && !me.email_verified && (
        <div>
          <Button size="sm" pending={resend.isPending} onClick={() => resend.mutate()}>
            {t('profile.address.resend')}
          </Button>
        </div>
      )}
      {me.email_pending !== null && <p className="text-sm text-text"><Trans i18nKey="profile.address.pending" values={{ email: me.email_pending }} components={{ address: <span data-user-content className="wrap-anywhere" /> }} /></p>}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" onClick={() => setChanging(true)}>
          {me.email === null ? t('profile.address.add') : t('profile.address.changeButton')}
        </Button>
        {(me.email !== null || me.email_pending !== null) && <Button size="sm" variant="ghost" disabled={clearReason !== null} onClick={() => setClearing(true)}>{t('profile.address.clear')}</Button>}
      </div>
      {(me.email !== null || me.email_pending !== null) && clearReason !== null && <p className="text-sm text-text-muted">{clearReason}</p>}
      <ChangeAddress me={me} open={changing} onClose={() => setChanging(false)} />
      {clearing && <ClearAddress me={me} onClose={() => setClearing(false)} />}
    </div>
  );
}

/** Name, with the picture it makes. */
function NameRow({ me }: { me: Me }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [left, setLeft] = useState(false);
  const busy = useIsMutating({ mutationKey: preferenceWriteKey(me.id), exact: true }) > 0;
  const save = useAutoSave({ current: { value: me.display_name }, blocked: busy, mutationKey: preferenceWriteKey(me.id), valid: (value) => value.trim() !== '' && Array.from(value.trim()).length <= DISPLAY_NAME_MAX_CHARS,
    write: async (value) => { const accepted = await updateMe({ display_name: value.trim() }); queryClient.setQueryData(meKey, accepted); return { value: accepted.display_name }; } });
  const name = save.value;
  const empty = name.trim() === '';
  const long = Array.from(name.trim()).length > DISPLAY_NAME_MAX_CHARS;
  const shown = avatarText(name) || me.avatar.text;
  return (
    <form
      noValidate
      className="flex flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault();
        setLeft(true);
        save.commit();
      }}
      onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) save.commit(); }} onCompositionStart={() => save.composing(true)} onCompositionEnd={() => save.composing(false)}
    >
      <div className="flex items-start gap-4">
        <Avatar text={shown} color={me.avatar.color} kind={me.kind === 'service' ? 'service' : 'human'} size={40} label={t('profile.name.preview')} />
        <Input
          label={t('profile.name.label')}
          hint={t('profile.name.hint')}
          value={name}
          autoComplete="name"
          error={long ? t('settings.auto.nameLong', { most: DISPLAY_NAME_MAX_CHARS }) : left && empty ? t('profile.name.required') : undefined}
          onBlur={() => setLeft(true)}
          onChange={(event) => save.edit(event.target.value)}
          fieldClassName="min-w-0 flex-1"
        />
      </div>
      <Input label={t('profile.username')} hint={t('profile.usernameHint')} value={me.username} readOnly mono fieldClassName="max-w-(--container-form)" />
      <AutoSaveFeedback save={save} name={t('profile.name.label')} />
    </form>
  );
}

/** The language, time zone, theme and density: shown as chosen at once, and put back if the server refuses. Drawn on
 * Interface, which owns them. */
const lookOf = (me: Me) => ({ locale: me.locale, timezone: me.timezone, timezone_mode: me.timezone_mode, theme: me.theme, density: me.density });
export function LookRows({ me }: { me: Me }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const set = useAppearance((state) => state.set);
  const busy = useIsMutating({ mutationKey: preferenceWriteKey(me.id), exact: true }) > 0;

  const save = useAutoSave({ current: { value: lookOf(me) }, blocked: busy, mutationKey: preferenceWriteKey(me.id), write: async (value, before) => {
    try {
      const accepted = await updateMe({
        ...(value.locale === before.value.locale ? {} : { locale: value.locale }),
        ...(value.timezone === before.value.timezone && value.timezone_mode === before.value.timezone_mode ? {} : { timezone: value.timezone, timezone_mode: value.timezone_mode }),
        ...(value.theme === before.value.theme ? {} : { theme: value.theme }),
        ...(value.density === before.value.density ? {} : { density: value.density }),
      });
      queryClient.setQueryData(meKey, accepted); adoptAppearance(accepted); return { value: lookOf(accepted) };
    } catch (error) {
      adoptAppearance(queryClient.getQueryData<Me>(meKey) ?? me);
      throw error;
    }
  } });
  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <Select
          label={t('preferences.language')}
          value={save.value.locale}
          disabled={save.pending}
          onValueChange={(value) => {
            set({ locale: value });
            save.choose((before) => ({ ...before, locale: value }));
          }}
          options={LOCALES.map((entry) => ({ value: entry.locale, ...choiceOption(t, `language.${entry.locale}`) }))}
        />
        <TimeZonePicker
          label={t('profile.look.timeZone')}
          value={save.value.timezone}
          mode={save.value.timezone_mode}
          disabled={save.pending}
          onValueChange={({ timeZone: chosen, mode }) => {
            set({ timeZone: chosen, timeZoneMode: mode });
            save.choose((before) => ({ ...before, timezone: chosen, timezone_mode: mode }));
          }}
        />
      </div>
      <div className="flex flex-col items-start gap-1">
      <span className="text-sm font-medium text-text">{t('preferences.theme')}</span>
      <SegmentedControl
        label={t('preferences.theme')}
        value={save.value.theme}
        onValueChange={(value) => {
          set({ theme: value });
          save.choose((before) => ({ ...before, theme: value }));
        }}
        segments={[
          { value: 'system', disabled: save.pending, icon: <Monitor aria-hidden />, name: t('profile.look.themeDevice') },
          { value: 'light', disabled: save.pending, icon: <Sun aria-hidden />, name: t('choiceCatalog.theme.light.name') },
          { value: 'dark', disabled: save.pending, icon: <Moon aria-hidden />, name: t('choiceCatalog.theme.dark.name') },
        ]}
      />
      </div>
      <div className="flex flex-col items-start gap-1">
      <span className="text-sm font-medium text-text">{t('preferences.density')}</span>
      <SegmentedControl
        label={t('preferences.density')}
        value={save.value.density}
        onValueChange={(value) => {
          set({ density: value });
          save.choose((before) => ({ ...before, density: value }));
        }}
        segments={[
          { value: 'comfortable', disabled: save.pending, name: t('choiceCatalog.density.comfortable.name') },
          { value: 'compact', disabled: save.pending, name: t('choiceCatalog.density.compact.name') },
        ]}
      />
      </div>
      <AutoSaveFeedback save={save} name={t('profile.sections.look')} />
    </div>
  );
}

export function Profile() {
  const { t } = useTranslation();
  const me = useQuery(meQuery);
  return (
    <PersonalFrame page="profile" lead={t('profile.lead')}>
      {me.isPending ? (
        <SkeletonRows rows={4} />
      ) : me.isError ? (
        <ErrorState error={me.error} retrying={me.isRefetching} onRetry={() => void me.refetch()} />
      ) : (
        <div className="flex flex-col gap-6">
          <Section id="profile-name" title={t('profile.sections.you')} lead={t('profile.sections.youLead')}>
            <div className={`${boxClasses} p-4`}>
              <NameRow key={`name-${me.data.id}`} me={me.data} />
              <AvatarColour key={`avatar-${me.data.id}`} me={me.data} />
            </div>
          </Section>
          <Section id="profile-address" title={t('profile.sections.address')} lead={t('profile.sections.addressLead')}>
            <div className={`${boxClasses} p-4`}>
              <AddressRow me={me.data} />
            </div>
          </Section>
        </div>
      )}
    </PersonalFrame>
  );
}
