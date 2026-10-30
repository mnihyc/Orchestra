/** The person's own page settings, with recorded-admin privacy wording and a reader-shaped preview. */
import { useIsMutating, useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { meQuery } from '@/api/auth';
import { ApiError } from '@/api/client';
import { profileKey, profileQuery, updateProfile } from '@/api/profile';
import { ProfileContents } from '@/features/profile/Page';
import { PinsEditor } from '@/features/profile/Pins';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { AutoSaveFeedback, useAutoSave } from '@/features/settings/AutoSave';
import type { ProfileAudience, UserProfile } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Textarea } from '@/ui/Textarea';

const LINK_LIMIT = 6; // The server accepts at most six links.
const pageOf = (page: UserProfile): { bio: string; pronouns: string; links: UserProfile['links']; showTime: boolean; showActivity: boolean; audience: ProfileAudience } => ({ bio: page.bio, pronouns: page.pronouns, links: page.links,
  showTime: page.settings?.show_time ?? true, showActivity: page.settings?.show_activity ?? true, audience: page.settings?.audience ?? 'system' });
function validPage(page: ReturnType<typeof pageOf>): boolean {
  return page.bio.length <= 2000 && page.pronouns.length <= 40 && page.links.length <= LINK_LIMIT && page.links.every((link) => {
    if (link.label.trim() === '' || link.label.length > 40 || link.url.length > 500) return false;
    try { const url = new URL(link.url); return url.protocol === 'https:' && url.hostname !== ''; } catch { return false; }
  });
}

function EditPage({ initial }: { initial: UserProfile }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const busy = useIsMutating({ mutationKey: profileKey(initial.user.username), exact: true }) > 0;
  const save = useAutoSave({ current: { value: pageOf(initial) }, valid: validPage, blocked: busy, mutationKey: profileKey(initial.user.username),
    write: async (value, before) => {
      const accepted = await updateProfile({
        ...(value.bio === before.value.bio ? {} : { bio: value.bio }), ...(value.pronouns === before.value.pronouns ? {} : { pronouns: value.pronouns }),
        ...(JSON.stringify(value.links) === JSON.stringify(before.value.links) ? {} : { links: value.links }),
        ...(value.showTime === before.value.showTime ? {} : { show_time: value.showTime }),
        ...(value.showActivity === before.value.showActivity ? {} : { show_activity: value.showActivity }),
        ...(value.audience === before.value.audience ? {} : { audience: value.audience }),
      });
      queryClient.setQueryData(profileKey(accepted.user.username), accepted); return { value: pageOf(accepted) };
    } });
  const { bio, pronouns, links, showTime, showActivity, audience } = save.value;
  const fault = (path: string) => save.error instanceof ApiError ? save.error.faults().find((fault) => fault.path === path)?.message : undefined;
  const preview = { ...initial, bio, pronouns, links: links.filter((link) => /^https:\/\//i.test(link.url)), local_time: showTime ? initial.local_time : null, is_me: false, can_message: false, settings: null, activity: showActivity };
  const audiences = (['system', 'orgs', 'private'] as const).map((value) => ({
    value, ...choiceOption(t, `profileAudience.${value}`),
    description: `${t(`profile.page.audience_${value}`)}${value === 'system' ? '' : ` ${t('profile.page.adminHelp')}`}`,
  }));
  return <div className="grid min-w-0 gap-6 xl:grid-cols-[minmax(18rem,26rem)_minmax(0,1fr)]"><div className="flex min-w-0 flex-col gap-6"><form className="flex flex-col gap-4"
    onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) save.commit(); }} onCompositionStart={() => save.composing(true)} onCompositionEnd={() => save.composing(false)}
    onSubmit={(event) => { event.preventDefault(); save.commit(); }}>
    <fieldset disabled={busy && !save.pending} className="flex min-w-0 flex-col gap-4">
    <Textarea label={t('profile.page.bio')} value={bio} maxLength={2000} error={fault('bio')} hint={t('profile.page.bioHelp')} onChange={(event) => save.edit({ ...save.value, bio: event.target.value })} />
    <Input label={t('profile.page.pronouns')} value={pronouns} maxLength={40} error={fault('pronouns')} onChange={(event) => save.edit({ ...save.value, pronouns: event.target.value })} />
    <fieldset className="flex flex-col gap-3"><legend className="mb-2 font-medium">{t('profile.page.links')}</legend>{links.map((link, index) => <div key={index} className="flex flex-col gap-2 rounded-lg border border-border p-3"><Input label={t('profile.page.linkLabel', { number: index + 1 })} value={link.label} maxLength={40} required error={fault(`links[${index}].label`)} onChange={(event) => save.edit({ ...save.value, links: links.map((entry, at) => at === index ? { ...entry, label: event.target.value } : entry) })} /><Input label={t('profile.page.linkUrl', { number: index + 1 })} type="url" value={link.url} maxLength={500} required pattern="https://.*" error={fault(`links[${index}].url`)} onChange={(event) => save.edit({ ...save.value, links: links.map((entry, at) => at === index ? { ...entry, url: event.target.value } : entry) })} /><Button size="sm" onClick={() => save.edit({ ...save.value, links: links.filter((_, at) => at !== index) })}>{t('profile.page.remove')}</Button></div>)}<Button size="sm" disabled={links.length >= LINK_LIMIT} onClick={() => save.edit({ ...save.value, links: [...links, { label: '', url: '' }] })}>{t('profile.page.addLink')}</Button></fieldset>
    <Checkbox label={t('profile.page.showTime')} checked={showTime} onCheckedChange={(value) => save.choose({ ...save.value, showTime: value })} />
    <Checkbox label={t('profile.page.showActivityToReaders')} checked={showActivity} onCheckedChange={(value) => save.choose({ ...save.value, showActivity: value })} />
    <Select label={t('profile.page.audience')} value={audience} onValueChange={(value) => save.choose({ ...save.value, audience: value })} options={audiences} />
    </fieldset>
    <AutoSaveFeedback save={save} name={t('profile.page.settingsTitle')} />
  </form><PinsEditor profile={initial} /></div><section className="min-w-0 rounded-lg border border-border bg-surface p-4" aria-label={t('profile.page.preview')}><h2 className="mb-4 text-lg font-semibold">{t('profile.page.preview')}</h2><ProfileContents profile={preview} preview /></section></div>;
}

function OwnPage({ username }: { username: string }) {
  const { t } = useTranslation();
  const profile = useQuery(profileQuery(username));
  if (profile.isPending) return <SkeletonRows rows={5} />;
  if (profile.isError) return <ErrorState error={profile.error} onRetry={() => void profile.refetch()} />;
  return <><a className="text-accent-text hover:underline" href={`/u/${encodeURIComponent(username)}`}>{t('profile.page.open')}</a><EditPage initial={profile.data} /></>;
}

export function ProfileSettingsPage() {
  const { t } = useTranslation();
  const me = useQuery(meQuery);
  return <PersonalFrame page="yourPage" lead={t('profile.page.settingsLead')}>{me.isPending ? <SkeletonRows rows={5} /> : me.isError ? <ErrorState error={me.error} onRetry={() => void me.refetch()} /> : <OwnPage username={me.data.username} />}</PersonalFrame>;
}
