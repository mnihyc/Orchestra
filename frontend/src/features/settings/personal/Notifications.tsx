/** Subscriptions, quiet hours, and Outside Orch: every way a person is told when they are not looking, each off until they switch it on.
 *
 * What the three levels of a place mean, whether to be told of approvals one can answer even where one is
 * not subscribed, how much of the chats one is in reaches one, the hours in which nothing is sent outside,
 * and the ways out themselves, each a card with its own switch. What a person chose about being notified is
 * theirs and easily undone, so a change shows at once and is put back if the server refuses.
 */
import { Hint } from '@/features/help/Hint';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { notificationPrefsKey, notificationPrefsQuery, setNotificationPrefs } from '@/api/notifications';
import { ALL_KINDS, KIND_LOOK } from '@/features/inbox/kinds';
import { CheckboxList } from '@/ui/Checkbox';
import { OutsideOrch } from '@/features/settings/personal/Routes';
import { PauseControls } from './away/Pause';
import { NotificationExceptions } from './NotificationExceptions';
import { AutoSaveFeedback, useAutoSave } from '@/features/settings/AutoSave';
import type { ChatLevel, MeNotificationPrefsSetBody, NotificationPrefs, QuietHours } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { Section } from '@/shell/Page';
import {PersonalFrame} from '@/features/settings/PersonalFrame';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { SettingRow } from '@/ui/SettingRow';

/** The hours quiet hours begin with when they are switched on: a night, from ten to seven. */
export const DEFAULT_QUIET: QuietHours = { start_minute: 22 * 60, end_minute: 7 * 60 };

/** The minutes after midnight a `HH:MM` text says; `null` for what is no time. */
export function minutesOfTime(text: string): number | null {
  const found = /^(\d{1,2}):(\d{2})$/.exec(text);
  if (found === null) return null;
  const hours = Number(found[1]);
  const minutes = Number(found[2]);
  return hours < 24 && minutes < 60 ? hours * 60 + minutes : null;
}

/** The `HH:MM` text of minutes after midnight, as a time field holds it. */
export function timeOfMinutes(minutes: number): string {
  return `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`;
}

const prefsValue = (prefs: NotificationPrefs) => ({ disabled_kinds: prefs.disabled_kinds ?? [], approvals_always: prefs.approvals_always, chat_node_level: prefs.chat_node_level,
  chat_direct_level: prefs.chat_direct_level, quiet_hours: prefs.quiet_hours });
/** What the person chose, with a captured native revision for automatic writes and Undo. */
function usePrefs() {
  const queryClient = useQueryClient();
  const query = useQuery(notificationPrefsQuery);
  const save = useAutoSave<ReturnType<typeof prefsValue> | null>({ current: { value: query.data === undefined ? null : prefsValue(query.data), revision: query.data?.revision }, enabled: query.data !== undefined,
    reload: () => query.refetch({ throwOnError: true }),
    write: async (value, before) => {
      if (value === null || typeof before.revision !== 'number') throw new Error('Notification preferences must be read before changing them');
      const accepted = await setNotificationPrefs({ expected_revision: before.revision,
        disabled_kinds: value.disabled_kinds, approvals_always: value.approvals_always, chat_node_level: value.chat_node_level, chat_direct_level: value.chat_direct_level, quiet_hours: value.quiet_hours });
      queryClient.setQueryData(notificationPrefsKey, accepted); return { value: prefsValue(accepted), revision: accepted.revision };
    } });
  return { query, save, change: (body: PrefsChange) => save.choose((held) => held === null ? null : ({ ...held,
    ...(body.approvals_always === undefined || body.approvals_always === null ? {} : { approvals_always: body.approvals_always }),
    ...(body.chat_node_level === undefined || body.chat_node_level === null ? {} : { chat_node_level: body.chat_node_level }),
    ...(body.chat_direct_level === undefined || body.chat_direct_level === null ? {} : { chat_direct_level: body.chat_direct_level }),
    ...(body.disabled_kinds === undefined || body.disabled_kinds === null ? {} : { disabled_kinds: body.disabled_kinds }),
    ...('quiet_hours' in body ? { quiet_hours: body.quiet_hours ?? null } : {}),
  })) };
}

/** A change of the person's choices, as the screen makes it: the revision is added when it is sent. */
type PrefsChange = Omit<MeNotificationPrefsSetBody, 'expected_revision'>;

const LEVELS: readonly ChatLevel[] = ['all', 'mentions', 'off'];

function ChatLevelSelect({ label, value, onChange }: { label: string; value: ChatLevel; onChange: (level: ChatLevel) => void }) {
  const { t } = useTranslation();
  return <SettingRow as="div" name={label} inset={false} value={<Select
      label={label}
      hideLabel
      value={value}
      onValueChange={onChange}
      fieldClassName="w-(--field-wide) max-w-full"
      options={LEVELS.map((level) => ({
        value: level,
        ...choiceOption(t, `subscriptionLevel.${level}`),
        description: level === 'all' ? t('notifications.chats.all') : level === 'mentions' ? t('notifications.chats.mentions') : t('notifications.chats.off'),
      }))}
    />} />;
}

export function Notifications() {
  const { t } = useTranslation();
  const { query, save, change } = usePrefs();
  const zone = useAppearance((appearance) => appearance.timeZone);
  const prefs = save.value;
  const quiet = prefs?.quiet_hours ?? null;

  return (
    <PersonalFrame page="notifications" lead={t('notifications.lead')}>

      <NotificationExceptions />

      {query.isPending ? (
        <SkeletonGroup className="flex flex-col gap-3">
          <Skeleton className="h-6 w-1/2" />
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-2/3" />
        </SkeletonGroup>
      ) : prefs === null ? (
        <ErrorState error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />
      ) : (
        <>
          <Section id="kinds" title={t('notifications.kinds')} action={<Hint popover id="loss-notify" />}>
            <CheckboxList label={t('notifications.kinds')} hideLabel presentation="compact"
              options={ALL_KINDS.map((kind) => ({ value: kind, label: t(KIND_LOOK[kind].label) }))}
              value={ALL_KINDS.filter((kind) => !prefs.disabled_kinds.includes(kind))}
              onValueChange={(enabled) => change({ disabled_kinds: ALL_KINDS.filter((kind) => !enabled.includes(kind)) })} />
          </Section>
          <Section id="approvals" title={t('notifications.approvals.title')}>
            <Switch presentation="compact" label={t('notifications.approvals.always')} description={t('notifications.approvals.hint')} checked={prefs.approvals_always} onCheckedChange={(on) => change({ approvals_always: on })} />
          </Section>

          <Section id="chats" title={t('notifications.chats.title')} lead={t('notifications.chats.lead')}>
            <div className="flex flex-col gap-4">
              <ChatLevelSelect label={t('notifications.chats.node')} value={prefs.chat_node_level} onChange={(level) => change({ chat_node_level: level })} />
              <ChatLevelSelect label={t('notifications.chats.direct')} value={prefs.chat_direct_level} onChange={(level) => change({ chat_direct_level: level })} />
            </div>
          </Section>

          <Section id="quiet" title={quiet === null ? t('notifications.quiet.title') : t('notifications.quiet.inZone', { zone })}>
            <div className="flex flex-col gap-3">
              <Switch
                presentation="compact"
                label={t('notifications.quiet.switch')}
                description={t('notifications.quiet.hint', { zone })}
                checked={quiet !== null}
                onCheckedChange={(on) => change({ quiet_hours: on ? DEFAULT_QUIET : null })}
              />
              {quiet !== null && (
                <div className="flex flex-wrap items-end gap-4">
                  <Input
                    label={t('notifications.quiet.from')}
                    type="time"
                    value={timeOfMinutes(quiet.start_minute)}
                    fieldClassName="w-(--field-narrow)"
                    onChange={(event) => {
                      const minutes = minutesOfTime(event.target.value);
                      if (minutes !== null) change({ quiet_hours: { ...quiet, start_minute: minutes } });
                    }}
                  />
                  <Input
                    label={t('notifications.quiet.until')}
                    type="time"
                    value={timeOfMinutes(quiet.end_minute)}
                    fieldClassName="w-(--field-narrow)"
                    onChange={(event) => {
                      const minutes = minutesOfTime(event.target.value);
                      if (minutes !== null) change({ quiet_hours: { ...quiet, end_minute: minutes } });
                    }}
                  />
                </div>
              )}
            </div>
          </Section>
          <AutoSaveFeedback save={save} name={t('screens.notifications.title')} />
        </>
      )}

      <Section id="outside" title={t('notifications.outside.title')} lead={t('notifications.outside.lead')}>
        <PauseControls />
        <OutsideOrch />
      </Section>
    </PersonalFrame>
  );
}
