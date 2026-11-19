/** Who may register; who may make a team org; confirming addresses; password sign-in; a code by mail, with its lock; how wrong tries are counted; and identity providers.
 *
 * The first choice is who may make an account here, as three cards, and the second who may make a team
 * org, as three more (an invitation to make one is given from Users); under them the settings that are plain
 * settings of the system, each changed where it is shown: confirming addresses (off where the system has
 * no mail server, and says why), whether a password signs people in at all, and what a code sent by mail
 * may be for, with the lock that holds every org to it. A line says how wrong tries are counted, since
 * people ask. The identity providers are listed below, with their forms.
 */
import { useQuery } from '@tanstack/react-query';
import { Info, Lock, LockOpen, ShieldUser, TicketPlus, UserCheck, UserRoundPlus, UsersRound } from 'lucide-react';
import { useId } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption, choiceCopy } from '@/i18n/choices';

import { identityProvidersQuery } from '@/api/admin';
import { metaQuery } from '@/api/auth';
import { IdentityProviders } from '@/features/admin/IdentityProviders';
import { SettingChangeStatus, SwitchSetting, useSettingChange, useSystemSettings } from '@/features/admin/SettingRows';
import { boxClasses, Section } from '@/shell/Page';
import { Checkbox } from '@/ui/Checkbox';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RadioGroup } from '@/ui/RadioGroup';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';

type Mode = 'open' | 'invite' | 'closed';

const MODES: readonly Mode[] = ['open', 'invite', 'closed'];
const ICONS = { open: <UsersRound aria-hidden />, invite: <UserCheck aria-hidden />, closed: <UserRoundPlus aria-hidden /> } as const;

type Makers = 'admins' | 'anyone' | 'invited';

const MAKERS: readonly Makers[] = ['admins', 'anyone', 'invited'];
const MAKER_ICONS = { admins: <ShieldUser aria-hidden />, anyone: <UsersRound aria-hidden />, invited: <TicketPlus aria-hidden /> } as const;

/** Team creation policy is independent of personal-home provisioning. */
function OrgMaking() {
  const { t } = useTranslation();
  const { settings } = useSystemSettings();
  const change = useSettingChange('orgs.created_by');
  if (settings.isPending) return <SkeletonRows rows={2} />;
  if (settings.isError) return <ErrorState compact error={settings.error} retrying={settings.isRefetching} onRetry={() => void settings.refetch()} />;
  // What was asked is shown while it is on its way, and the old choice comes back if the system refuses.
  const held = MAKERS.find((makers) => makers === change.save.variables.value) ?? null;
  return (
    <div data-setting="orgs.created_by" className="flex flex-col gap-3">
      <RadioGroup<Makers>
        presentation="compact"
        label={t('admin.signIn.orgs.label')}
        value={held}
        disabled={change.pending || !change.writable}
        onValueChange={(makers) => change.save.mutate({ value: makers })}
        options={MAKERS.map((makers) => ({
          value: makers,
          ...choiceOption(t, `orgCreation.${makers}`),
          description: t(`admin.signIn.orgs.${makers}.about`),
          icon: MAKER_ICONS[makers],
          ...(makers === 'invited' ? { note: t('admin.signIn.registration.asShipped') } : {}),
        }))}
      />
      {held === 'invited' && <p className="text-sm text-text-muted">{t('admin.signIn.orgs.whereInvitations')}</p>}
      <p className="text-sm text-text-muted">{t('admin.signIn.orgs.ownOrg')}</p>
      <SettingChangeStatus change={change} name={t('admin.signIn.orgs.label')} />
    </div>
  );
}

/** Who may make an account: three cards, and the line that holds for all of them. */
function Registration() {
  const { t } = useTranslation();
  const { settings } = useSystemSettings();
  const change = useSettingChange('registration.mode');
  if (settings.isPending) return <SkeletonRows rows={2} />;
  if (settings.isError) return <ErrorState compact error={settings.error} retrying={settings.isRefetching} onRetry={() => void settings.refetch()} />;
  // What was asked is shown while it is on its way, and the old choice comes back if the system refuses.
  const held = MODES.find((mode) => mode === change.save.variables.value) ?? null;
  return (
    <div data-setting="registration.mode" className="flex flex-col gap-3">
      <RadioGroup<Mode>
        presentation="compact"
        label={t('admin.signIn.registration.label')}
        value={held}
        disabled={change.pending || !change.writable}
        onValueChange={(mode) => change.save.mutate({ value: mode })}
        options={MODES.map((mode) => ({
          value: mode,
          ...choiceOption(t, `registration.${mode}`),
          description: t(`admin.signIn.registration.${mode}.about`),
          icon: ICONS[mode],
          ...(mode === 'invite' ? { note: t('admin.signIn.registration.asShipped') } : {}),
        }))}
      />
      <SwitchSetting settingKey="orgs.personal_enabled" label={t('settingRegistry.orgs.personal_enabled.title')} hint={t('settingRegistry.orgs.personal_enabled.description')} />
      <SettingChangeStatus change={change} name={t('admin.signIn.registration.label')} />
    </div>
  );
}

/** What a code sent by mail may be for: a way in without the password, a second step after it, both or neither; and the lock that holds every org to it. */
function MailedCode({ noMail }: { noMail: boolean }) {
  const { t } = useTranslation();
  const { find } = useSystemSettings();
  const change = useSettingChange('auth.email_code');
  const setting = find('auth.email_code');
  const heading = useId();
  if (setting === undefined) return null;
  // What was asked is shown while it is on its way, and the old values come back if the system refuses.
  const shown = change.save.variables;
  const uses = Array.isArray(shown.value) ? (shown.value as unknown[]).filter((use): use is string => typeof use === 'string') : [];
  const locked = shown.locked;
  const toggle = (use: 'sign_in' | 'second_step', on: boolean) => {
    const next = on ? [...new Set([...uses, use])] : uses.filter((one) => one !== use);
    change.save.mutate({ value: next, locked });
  };
  const off = change.pending || !change.writable || noMail;
  return (
    <div role="group" aria-labelledby={heading} data-setting="auth.email_code" className={cx(boxClasses, 'flex flex-col gap-3 p-4')}>
      <div className="flex flex-col gap-1">
        <h3 id={heading} className="text-base font-medium text-text">
          {t('admin.signIn.code.title')}
        </h3>
        <p className="text-sm text-text-muted">{noMail ? t('admin.signIn.noMail') : t('admin.signIn.code.lead')}</p>
      </div>
      <Checkbox presentation="compact" label={choiceCopy(t, 'emailCode.sign_in').name} description={t('admin.signIn.code.signIn')} checked={uses.includes('sign_in')} disabled={off} onCheckedChange={(on) => toggle('sign_in', on)} />
      <Checkbox presentation="compact" label={choiceCopy(t, 'emailCode.second_step').name} description={t('admin.signIn.code.secondStep')} checked={uses.includes('second_step')} disabled={off} onCheckedChange={(on) => toggle('second_step', on)} />
      <p className="text-sm text-text-muted">{t('admin.signIn.code.safety')}</p>
      <Switch
        label={
          <span className="flex items-center gap-2">
            {locked ? <Lock aria-hidden /> : <LockOpen aria-hidden />}
            {t('admin.signIn.code.lock')}
          </span>
        }
        hint={t('admin.signIn.code.lockHint')}
        checked={locked}
        disabled={change.pending || !change.writable}
        onCheckedChange={(next) => change.save.mutate({ value: uses, locked: next })}
      />
      <SettingChangeStatus change={change} name={t('admin.signIn.code.title')} />
    </div>
  );
}

export function SignIn() {
  const { t } = useTranslation();
  const meta = useQuery(metaQuery).data;
  const providers = useQuery(identityProvidersQuery);
  const { find } = useSystemSettings();
  const noMail = meta?.mail === false;
  const passwords = find('auth.local_login')?.value !== false;
  const anyProvider = (providers.data ?? []).some((provider) => provider.enabled);

  return (
    <div className="flex flex-col gap-6">
      <Section id="admin-registration" title={t('admin.signIn.registration.title')} lead={t('admin.signIn.registration.lead')}>
        <Registration />
      </Section>

      <Section id="admin-org-making" title={t('admin.signIn.orgs.title')} lead={t('admin.signIn.orgs.lead')}>
        <OrgMaking />
      </Section>

      <Section id="admin-proofs" title={t('admin.signIn.proofs.title')} lead={t('admin.signIn.proofs.lead')}>
        <div className={cx(boxClasses, 'flex flex-col gap-6 p-4')}>
          <SwitchSetting
            settingKey="registration.email_verification"
            words={['off', 'required']}
            label={t('admin.signIn.verify.label')}
            hint={t('admin.signIn.verify.hint')}
            off={noMail ? t('admin.signIn.verify.noMail') : undefined}
          />
          <SwitchSetting settingKey="auth.local_login" label={t('admin.signIn.password.label')} hint={t('admin.signIn.password.hint')} />
          {!passwords && !anyProvider && providers.isSuccess && <InlineAlert tone="warning">{t('admin.signIn.password.nobody')}</InlineAlert>}
        </div>
        <MailedCode noMail={noMail} />
        <p className="flex items-start gap-2 text-sm text-text-muted">
          <Info aria-hidden className="mt-0.5 shrink-0" />
          {t('admin.signIn.tries')}
        </p>
      </Section>

      <Section id="admin-providers" title={t('admin.signIn.providers.title')} lead={t('admin.signIn.providers.lead')}>
        <IdentityProviders />
      </Section>
    </div>
  );
}
