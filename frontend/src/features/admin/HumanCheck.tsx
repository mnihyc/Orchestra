/** The human check of the sign-in pages: which service asks, with which keys, and where; and the secret, which is written and never read.
 *
 * One card under Management → Sign-in. Off, only the choice of service shows. Chosen, the controls that
 * service has show, each a plain setting of the system changed where it is shown; the secret is the one
 * thing here that is not a setting: it is kept sealed, set with its own button, and said to be kept or
 * not, and **Test** asks the service with a token that is not one, to see that it is reached and
 * accepts the keys.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { deleteHumanCheckSecret, humanCheckKey, humanCheckQuery, setHumanCheckSecret, testHumanCheck } from '@/api/admin';
import { clearSetting, putSetting } from '@/api/settings';
import {
  acceptSystemSetting,
  ChoiceSetting,
  NumberSetting,
  SettingChangeStatus,
  SwitchSetting,
  useSettingChange,
  useSystemSettings,
} from '@/features/admin/SettingRows';
import { AutoSaveFeedback, useAutoSave } from '@/features/settings/AutoSave';
import type { HumanCheckAct } from '@/generated/api-types';
import { choiceCopy, choiceOption } from '@/i18n/choices';
import { useAppearance } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { boxClasses } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';

const ACTS: readonly HumanCheckAct[] = ['sign_in', 'email_code', 'register', 'recovery', 'resend'];

/** A short public text of the system's settings, saved when its writer rests or leaves it. */
function TextSetting({ settingKey, label }: { settingKey: string; label: string }) {
  const { find, settings } = useSystemSettings();
  const cache = useQueryClient();
  const setting = find(settingKey);
  const own = (row: typeof setting) => (row?.own === null || row?.own === undefined ? row?.value : row.own.value);
  const snapshot = (row: typeof setting) => ({ text: typeof own(row) === 'string' ? (own(row) as string) : '' });
  const save = useAutoSave({
    current: { value: snapshot(setting), revision: setting?.own?.revision ?? null },
    enabled: setting?.writable === true,
    reload: () => settings.refetch({ throwOnError: true }),
    write: async (value, before) => {
      const revision = typeof before.revision === 'number' ? before.revision : null;
      const text = value.text.trim();
      const accepted = text === ''
        ? await clearSetting({ kind: 'system' }, settingKey, revision)
        : await putSetting({ kind: 'system' }, settingKey, { value: text, locked: false, expectedRevision: revision });
      acceptSystemSetting(cache, accepted);
      return { value: snapshot(accepted), revision: accepted.own?.revision ?? null };
    },
  });
  if (setting === undefined) return null;
  return (
    <div data-item data-setting={settingKey} className="flex flex-col gap-1">
      <Input
        label={label}
        value={save.value.text}
        autoComplete="off"
        autoCapitalize="none"
        spellCheck={false}
        disabled={!setting.writable}
        onChange={(event) => save.edit({ text: event.target.value })}
        onBlur={() => save.commit()}
        fieldClassName="max-w-(--container-form)"
      />
      <AutoSaveFeedback save={save} name={label} />
    </div>
  );
}

/** Which acts ask for the check: five boxes, saved at once. */
function Acts() {
  const { t } = useTranslation();
  const { find } = useSystemSettings();
  const change = useSettingChange('human_check.acts');
  const setting = find('human_check.acts');
  if (setting === undefined) return null;
  const shown = change.save.variables.value;
  const asked = Array.isArray(shown) ? (shown as unknown[]).filter((act): act is string => typeof act === 'string') : [];
  const toggle = (act: HumanCheckAct, on: boolean) => {
    const next = ACTS.filter((each) => (each === act ? on : asked.includes(each)));
    change.save.mutate({ value: next });
  };
  return (
    <div role="group" aria-label={t('admin.signIn.humanCheck.acts')} data-setting="human_check.acts" className="flex flex-col gap-2">
      <span className="text-sm font-medium text-text">{t('admin.signIn.humanCheck.acts')}</span>
      {ACTS.map((act) => (
        <Checkbox
          key={act}
          presentation="compact"
          label={choiceCopy(t, `humanCheckAct.${act}`).name}
          checked={asked.includes(act)}
          disabled={change.pending || !change.writable}
          onCheckedChange={(on) => toggle(act, on)}
        />
      ))}
      <SettingChangeStatus change={change} name={t('admin.signIn.humanCheck.acts')} />
    </div>
  );
}

/** The secret: written here, sealed, never read back; and a test of the keys. */
function Secret({ way }: { way: 'hcaptcha' | 'siteverify' | 'assessments' }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const state = useQuery(humanCheckQuery);
  const [typed, setTyped] = useState('');
  const id = useId();
  const label = t(`admin.signIn.humanCheck.secret.${way}`);
  const keep = useMutation({
    mutationFn: () => setHumanCheckSecret(typed),
    onSuccess: (kept) => {
      setTyped('');
      cache.setQueryData(humanCheckKey, kept);
    },
  });
  const drop = useMutation({ mutationFn: deleteHumanCheckSecret, onSuccess: (kept) => cache.setQueryData(humanCheckKey, kept) });
  const test = useMutation({ mutationFn: testHumanCheck });
  const kept = state.data?.secret_set === true;
  return (
    <div data-item data-human-check-secret className="flex flex-col gap-2">
      <form
        className="flex flex-col gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          if (typed.trim() !== '' && !keep.isPending) keep.mutate();
        }}
      >
        <PasswordInput
          label={label}
          value={typed}
          placeholder={t('admin.signIn.humanCheck.secretPlaceholder')}
          autoComplete="off"
          aria-describedby={id}
          onChange={(event) => setTyped(event.target.value)}
          fieldClassName="w-full max-w-[20rem]"
        />
        <div className="flex flex-wrap items-center gap-2">
          <Button type="submit" size="sm" variant="primary" pending={keep.isPending} disabled={typed.trim() === ''}>
            {t('admin.signIn.humanCheck.set')}
          </Button>
          {kept && (
            <Button type="button" size="sm" variant="ghost" pending={drop.isPending} onClick={() => drop.mutate()}>
              {t('admin.signIn.humanCheck.remove')}
            </Button>
          )}
          <Button type="button" size="sm" variant="secondary" pending={test.isPending} disabled={!kept} onClick={() => test.mutate()}>
            {test.isPending ? t('admin.signIn.humanCheck.testing') : t('admin.signIn.humanCheck.test')}
          </Button>
        </div>
      </form>
      <p id={id} className="text-sm text-text-muted">
        {kept && state.data?.secret_changed_at !== null && state.data?.secret_changed_at !== undefined
          ? t('admin.signIn.humanCheck.secretKept', { time: formatDateTime(state.data.secret_changed_at, { locale, timeZone }) })
          : t('admin.signIn.humanCheck.secretNone')}
      </p>
      {keep.error !== null && <ErrorAlert error={keep.error} />}
      {drop.error !== null && <ErrorAlert error={drop.error} />}
      {test.error !== null && <ErrorAlert error={test.error} />}
      {test.data !== undefined && (test.data.ok
        ? <InlineAlert tone="success">{t('admin.signIn.humanCheck.testOk')}</InlineAlert>
        : <InlineAlert tone="warning">{test.data.error ?? ''}</InlineAlert>)}
    </div>
  );
}

/**
 * What the web edge's content policy leaves the service chosen: nothing is said where it has room, which is how it is meant
 * to be; where it has none, the hosts it lacks (the page cannot load the widget, and the server does not switch the check on
 * for it); where the server could not read it, that.
 */
function EdgeNote({ service, alternate }: { service: 'hcaptcha' | 'recaptcha'; alternate: boolean }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const edge = useQuery(humanCheckQuery).data?.edge;
  // The server looks at the edge anew whenever it is asked; it is asked again when the service or the address it is loaded from changes.
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    void cache.invalidateQueries({ queryKey: humanCheckKey });
  }, [cache, service, alternate]);
  if (edge === null || edge === undefined || edge.state === 'allows') return null;
  if (edge.state === 'unknown') return <p className="text-sm text-text-muted">{t('admin.signIn.humanCheck.edgeUnknown')}</p>;
  const hosts = [...new Set(edge.lacking.map((one) => one.split(' in ')[0] ?? one))].join(', ');
  return <InlineAlert tone="warning">{t('admin.signIn.humanCheck.edgeBlocks', { service: choiceCopy(t, `humanCheckProvider.${service}`).name, hosts })}</InlineAlert>;
}

/** The card. */
export function HumanCheckCard() {
  const { t } = useTranslation();
  const { find, settings } = useSystemSettings();
  const provider = find('human_check.provider')?.value;
  const verification = find('human_check.verification')?.value;
  const siteKey = find('human_check.site_key')?.value;
  if (settings.isPending || find('human_check.provider') === undefined) return null;
  const hcaptcha = provider === 'hcaptcha';
  const recaptcha = provider === 'recaptcha';
  const assessments = verification === 'assessments';
  const way = hcaptcha ? 'hcaptcha' : assessments ? 'assessments' : 'siteverify';
  const option = (group: 'humanCheckProvider' | 'humanCheckWidget' | 'humanCheckVerification' | 'humanCheckUnreachable', value: string) => ({
    value,
    ...choiceOption(t, `${group}.${value}` as never),
  });
  return (
    <div data-setting="human_check.provider" className={cx(boxClasses, 'flex flex-col gap-4 p-4')}>
      <ChoiceSetting
        settingKey="human_check.provider"
        label={t('admin.signIn.humanCheck.service')}
        options={['off', 'hcaptcha', 'recaptcha'].map((value) => option('humanCheckProvider', value))}
      />
      {(hcaptcha || recaptcha) && (
        <>
          <TextSetting settingKey="human_check.site_key" label={t('admin.signIn.humanCheck.siteKey')} />
          {typeof siteKey === 'string' && siteKey.trim() === '' && <InlineAlert tone="warning">{t('admin.signIn.humanCheck.noKey')}</InlineAlert>}
          {hcaptcha && (
            <ChoiceSetting
              settingKey="human_check.mode"
              label={t('admin.signIn.humanCheck.widget')}
              options={['checkbox', 'invisible'].map((value) => option('humanCheckWidget', value))}
            />
          )}
          {/* A mode left over from an earlier hCaptcha choice is neither shown nor applied here. */}
          {recaptcha && <p className="text-sm text-text-muted">{t('admin.signIn.humanCheck.unseen')}</p>}
          {recaptcha && (
            <ChoiceSetting
              settingKey="human_check.verification"
              label={t('admin.signIn.humanCheck.verification')}
              options={['siteverify', 'assessments'].map((value) => option('humanCheckVerification', value))}
            />
          )}
          {recaptcha && assessments && <TextSetting settingKey="human_check.project_id" label={t('admin.signIn.humanCheck.project')} />}
          <Secret way={way} />
          {recaptcha && (
            <NumberSetting settingKey="human_check.min_score" label={t('admin.signIn.humanCheck.leastScore')} min={0} max={100} unit={t('admin.signIn.humanCheck.scoreUnit')} />
          )}
          {recaptcha && <SwitchSetting settingKey="human_check.alternate_host" label={t('admin.signIn.humanCheck.alternate')} />}
          <Acts />
          <ChoiceSetting
            settingKey="human_check.when_unreachable"
            label={t('admin.signIn.humanCheck.unreachable')}
            options={['admit', 'refuse'].map((value) => option('humanCheckUnreachable', value))}
          />
          <EdgeNote service={hcaptcha ? 'hcaptcha' : 'recaptcha'} alternate={recaptcha && find('human_check.alternate_host')?.value === true} />
        </>
      )}
    </div>
  );
}
