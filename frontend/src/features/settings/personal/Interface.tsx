/** What the interface keeps for a person on every device: the viewer each kind of file opens with, the tutorials finished and the hints closed.
 *
 * Besides those, the page lists what this device keeps for the person alone. The first three follow the person (`GET /me/state`) and are changed here as they are anywhere: the
 * whole value of a key is read, one entry of it is changed, and the whole value is written back, so
 * that an entry this page does not know is never lost. What a device keeps for itself (the sizes and
 * places of panes, the tabs that were open) is listed with what it comes to and can be forgotten,
 * which changes nothing for the person on any other device.
 */
import { forgetViewChoiceDeviceData, viewChoiceKeyDetails } from '@/lib/viewChoices';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { GraduationCap, HardDrive, Keyboard, RotateCcw } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { useStore } from 'zustand';

import { meQuery } from '@/api/auth';
import { client } from '@/api/client';
import { meStateKey, meStateQuery, setMeState } from '@/api/personal';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { readHelpState } from '@/features/help/Dismissed';
import { forgetCommentDraftDeviceData } from '@/features/documents/commentDrafts';
import { TUTORIALS as SHIPPED_TUTORIALS, tutorialOf } from '@/help/tutorials';
import { useSaveKeys } from '@/features/help/KeyList';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { LookRows } from '@/features/settings/personal/Profile';
import type { ScreenKind } from '@/routes/-access';
import { requestKeyList, useKeyChoices } from '@/shell/keys';
import { boxClasses, Section } from '@/shell/Page';
import { actionName } from '@/ui/actions';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { Input } from '@/ui/Input';
import { READING_STATE_KEY, useInterfaceReading } from '@/lib/interfaceReading';
import { linkSharingPreference } from '@/features/sharing/preference';
import { Select } from '@/ui/Select';
import { LINK_OPENINGS, useSidebarPreferences } from '@/features/navigation/preferences';

/** The tutorials that ship, by the name each is kept under when it is finished. */
export const TUTORIALS = [
  'first-thread',
  'step-in',
  'share-a-thread',
  'permission-page',
  'connect-a-computer',
  'add-a-provider',
  'code-beside-an-agent',
  'write-together',
  'what-the-team-knows',
  'usage-and-limits',
] as const;

export type Tutorial = (typeof TUTORIALS)[number];


/** The kinds of screen whose layout a device keeps. */
/** Known storage fields are translated through these catalogue keys, never shown as implementation names. */
const CHOICE_KINDS = ['global', 'org', 'project', 'topic', 'thread', 'document', 'page', 'resource-providers', 'resource-agents', 'resource-environments', 'resource-hosts', 'resource-machines', 'resource-templates', 'provider-endpoints', 'chat', 'help', 'search', 'approvals', 'ports', 'storage', 'trace', 'navigation', 'onboarding', 'links', 'import-review', 'import-report', 'usage'] as const;
const CHOICE_FIELDS = ['tab', 'view', 'rightOpen', 'rightSize', 'mobileView', 'mobileTab', 'listOrigin', 'scope', 'advanced', 'endpointAdvanced', 'part', 'setup', 'filters'] as const;

const SCREEN_KINDS: readonly ScreenKind[] = ['org', 'project', 'topic', 'thread', 'document', 'page'];

const isScreenKind = (name: string): name is ScreenKind => (SCREEN_KINDS as readonly string[]).includes(name);

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The names held by a value that is a list of names, a list of entries with a name, or a map by name. */
export function namesIn(value: unknown): string[] {
  if (Array.isArray(value)) {
    const list: unknown[] = value;
    return list.flatMap((entry): string[] => {
      if (typeof entry === 'string') return [entry];
      if (isRecord(entry) && typeof entry.name === 'string') return [entry.name];
      return [];
    });
  }
  if (isRecord(value)) return Object.keys(value);
  return [];
}

/** The same value without one name, in the shape it had. */
export function withoutName(value: unknown, name: string): unknown {
  if (Array.isArray(value)) {
    const list: unknown[] = value;
    return list.filter((entry) => !(entry === name || (isRecord(entry) && entry.name === name)));
  }
  if (isRecord(value)) return Object.fromEntries(Object.entries(value).filter(([key]) => key !== name));
  return value;
}

/** The same value with nothing in it, in the shape it had. */
export function emptied(value: unknown): unknown {
  return Array.isArray(value) ? [] : isRecord(value) ? {} : value;
}

/** What one key of the person's state holds, as the page reads it. */
function entryOf(state: Record<string, unknown> | undefined, key: string): unknown {
  return state?.[key];
}

/** What a person has finished and closed, as the help keeps it under `help`: the tutorials finished and the tips closed, each as names. */
export function helpOf(state: Record<string, unknown> | undefined): { tutorials: string[]; hints: string[] } {
  const held = readHelpState(entryOf(state, 'help'));
  return { tutorials: held.finished, hints: held.closed };
}

/** The part of `help` that each of the two lists is kept in. */
const KEPT_IN = { tutorials: 'finished', hints: 'closed' } as const;

/** Writes one key of the state: the value as it stands in the cache is changed by `change`, and the whole of it is sent. */
function useStateKey(key: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (change: (held: unknown) => unknown) => {
      const reader = libraryReader();
      const state = queryClient.getQueryData<Record<string, unknown>>(meStateKey);
      const next = change(entryOf(state, key));
      await setMeState(key, next);
      return { next, reader };
    },
    onSuccess: ({ next, reader }) => {
      if (!sameLibraryReader(reader)) return;
      queryClient.setQueryData<Record<string, unknown>>(meStateKey, (state) => ({ ...state, [key]: next }));
    },
    onSettled: (_data, error) => { if (error !== null) void queryClient.invalidateQueries({ queryKey: meStateKey }); },
  });
}

/** Whether a hidden bar peeks over the page when the pointer rests at its edge: off unless the person turns it on, and kept with the other choices of the bars. */
function OpenLinks() {
  const { t } = useTranslation();
  const choices = useSidebarPreferences();
  return <Section id="interface-opening" title={t('workbench.openLinks.label')}>
    <Select size="sm" label={t('workbench.openLinks.label')} value={choices.openLinks ?? 'automatic'} onValueChange={choices.setOpenLinks}
      options={LINK_OPENINGS.map((value) => ({ value, name: t(`workbench.openLinks.${value}`) }))} />
  </Section>;
}

function HiddenBars() {
  const { t } = useTranslation();
  const preferences = useSidebarPreferences();
  return <Section id="interface-bars" title={t('workbench.bars.peek.title')}>
    <Switch label={t('workbench.bars.peek.label', { state: t(preferences.peek === true ? 'common.on' : 'common.off') })} checked={preferences.peek === true} onCheckedChange={preferences.setPeek} />
  </Section>;
}

function Reading() {
  const { t } = useTranslation();
  const reading = useInterfaceReading();
  const write = useStateKey(READING_STATE_KEY);
  const [recent, setRecent] = useState(String(reading.kept_closed_tabs));
  const [delay, setDelay] = useState(String(reading.progress_delay_ms));
  const save = () => {
    const kept_closed_tabs = Number(recent), progress_delay_ms = Number(delay);
    if (recent.trim() === '' || delay.trim() === '' || ![kept_closed_tabs, progress_delay_ms].every((value) => Number.isSafeInteger(value) && value >= 0)) return;
    write.mutate((held) => ({ ...(isRecord(held) ? held : {}), kept_closed_tabs, progress_delay_ms }));
  };
  return <Section id="interface-reading" title={t('settings.interface.reading.title')}>
    <div className="flex flex-wrap items-end gap-3">
      <Input label={t('settings.interface.reading.recent')} type="number" min={0} step={1} value={recent} onChange={(event) => setRecent(event.target.value)} fieldClassName="w-44" />
      <Input label={t('settings.interface.reading.delay')} type="number" min={0} step={1} value={delay} onChange={(event) => setDelay(event.target.value)} fieldClassName="w-44" />
      <Button size="sm" pending={write.isPending} onClick={save}>{t('common.save')}</Button>
    </div>
    {write.error !== null && <ErrorAlert error={write.error} />}
  </Section>;
}

function LinkSharing({ state }: { state: Record<string, unknown> }) {
  const { t } = useTranslation();
  const write = useStateKey('timeline');
  const checked = linkSharingPreference(state);
  return <Section id="interface-links" title={t('linkSharing.settingsTitle')}>
    {checked === null ? <span className="text-sm text-text-muted">{t('linkSharing.unavailable')}</span>
      : <Switch label={t('linkSharing.preference')} checked={checked} disabled={write.isPending}
        onCheckedChange={(on) => write.mutate((held) => ({ ...(isRecord(held) ? held : {}), share_on_link: on }))} />}
    {write.error !== null && <ErrorAlert error={write.error} />}
  </Section>;
}

// ---------------------------------------------------------------- what each kind of file opens with

function Viewers({ state }: { state: Record<string, unknown> | undefined }) {
  const { t } = useTranslation();
  const held = entryOf(state, 'viewers');
  const chosen = isRecord(held) ? Object.entries(held).filter(([, viewer]) => typeof viewer === 'string') : [];
  const write = useStateKey('viewers');
  return (
    <Section id="interface-viewers" title={t('settings.interface.viewers.title')}>
      {chosen.length === 0 ? (
        <EmptyState compact title={t('settings.interface.viewers.none')} />
      ) : (
        <ul className={`${boxClasses} divide-y divide-border`}>
          {chosen.map(([kind, viewer]) => (
            <li key={kind} className="flex items-center gap-3 px-4 py-2">
              <span className="min-w-0 flex-1 text-base text-text">
                <span className="font-mono">{kind}</span>{' '}
                <span className="text-text-muted">{t('settings.interface.viewers.opensWith', { viewer: String(viewer) })}</span>
              </span>
              <Button size="sm" variant="ghost" icon={<RotateCcw aria-hidden />} pending={write.isPending} onClick={() => write.mutate((value) => withoutName(value, kind))}>
                {t('settings.interface.viewers.useDefault')}
              </Button>
            </li>
          ))}
        </ul>
      )}
      {write.error !== null && <ErrorAlert error={write.error} />}
    </Section>
  );
}

// ---------------------------------------------------------------- tutorials and hints

function Tutorials({ state }: { state: Record<string, unknown> | undefined }) {
  const { t } = useTranslation();
  const { tutorials: finished, hints } = helpOf(state);
  const write = useStateKey('help');
  const ids = [...SHIPPED_TUTORIALS.filter((one) => finished.includes(one.id)).map((one) => one.id), ...finished.filter((id) => tutorialOf(id) === undefined)];
  const title = (id: string) => { const tutorial = tutorialOf(id); return tutorial === undefined ? id : t(tutorial.title); };
  // `help` is one key with two parts: changing one of them keeps the other as it was.
  const change = (part: 'tutorials' | 'hints', alter: (held: unknown) => unknown) =>
    write.mutate((help) => ({ ...(isRecord(help) ? help : {}), [KEPT_IN[part]]: alter(isRecord(help) ? help[KEPT_IN[part]] : undefined) }));
  return (
    <Section id="interface-help" title={t('settings.interface.help.title')} lead={t('settings.interface.help.lead')}>
      <div className={`${boxClasses} flex flex-col divide-y divide-border`}>
        {ids.length === 0 ? (
          <p className="flex items-center gap-3 px-4 py-3 text-sm text-text-muted">
            <GraduationCap className="size-4 shrink-0 text-text-subtle" aria-hidden />
            {t('settings.interface.help.noTutorials')}
          </p>
        ) : (
          <ul className="divide-y divide-border">
            {ids.map((id) => (
              <li key={id} className="flex items-center gap-3 px-4 py-2">
                <GraduationCap className="size-4 shrink-0 text-text-subtle" aria-hidden />
                <span className="min-w-0 flex-1 text-base text-text">{title(id)}</span>
                <Button size="sm" variant="ghost" pending={write.isPending} onClick={() => change('tutorials', (held) => withoutName(held, id))}>
                  {t('settings.interface.help.showAgain')}
                </Button>
              </li>
            ))}
          </ul>
        )}
        <div className="flex flex-wrap items-center gap-3 px-4 py-3">
          <p className="min-w-0 flex-1 text-base text-text">{hints.length === 0 ? t('settings.interface.help.noHints') : t('settings.interface.help.hintsClosed', { count: hints.length })}</p>
          <Button size="sm" blocked={hints.length === 0} pending={write.isPending} onClick={() => change('hints', emptied)}>
            {t('settings.interface.help.bringBack')}
          </Button>
        </div>
      </div>
      {write.error !== null && <ErrorAlert error={write.error} />}
    </Section>
  );
}

// ---------------------------------------------------------------- keys

/** Whether keys without a modifier act, and the way to the list where each action's keys are changed. Kept with the person (`keys`). */
function Keys() {
  const { t } = useTranslation();
  const choices = useKeyChoices();
  const save = useSaveKeys();
  // While a change is being saved the switch shows it; a change that fails goes back to what is kept.
  const shown = save.isPending ? (save.variables.single_key ?? choices.single_key) : choices.single_key;
  return (
    <Section id="interface-keys" title={t('shell.keyChoice.single.title')} lead={t('shell.keyChoice.single.lead')}>
      <div className={`${boxClasses} flex flex-col gap-3 px-4 py-3`}>
        <Switch
          label={t(shown ? 'shell.keyChoice.single.labelOn' : 'shell.keyChoice.single.labelOff')}
          checked={shown}
          disabled={save.isPending}
          onCheckedChange={(on) => save.mutate({ single_key: on })}
        />
        <div>
          <Button size="sm" icon={<Keyboard aria-hidden />} onClick={() => requestKeyList()}>
            {t('shell.keyChoice.single.editor')}
          </Button>
        </div>
      </div>
      {save.error !== null && (
        <ErrorAlert
          error={save.error}
          action={
            <Button size="sm" onClick={() => save.mutate(save.variables)}>
              {t('shell.keyChoice.retry')}
            </Button>
          }
        />
      )}
    </Section>
  );
}

// ---------------------------------------------------------------- what this device keeps

/** One thing a device keeps for a person: where, and about how much. */
export interface KeptItem {
  key: string;
  bytes: number;
}

export interface KeptGroup extends KeptItem {
  id: string;
  keys: string[];
  choice: { kind: string; key: string | null } | null;
}

/** One named category clears its own choices, including deliberate per-page overrides. */
export function deviceKeptGroups(userId: string | null, items: readonly KeptItem[]): KeptGroup[] {
  if (userId === null) return [];
  const groups = new Map<string, KeptGroup>();
  for (const item of items) {
    if (!item.key.startsWith(`orch.${userId}.`)) continue;
    const choice = viewChoiceKeyDetails(userId, item.key);
    const known = CHOICE_KINDS.some((kind) => kind === choice?.kind);
    const id = choice === null ? item.key : known ? `choices:${choice.kind}` : 'choices:other';
    const before = groups.get(id);
    if (before === undefined) groups.set(id, { ...item, id, keys: [item.key], choice });
    else {
      before.keys.push(item.key); before.bytes += item.bytes;
      if (before.choice !== null && before.choice.key !== choice?.key) before.choice.key = null;
    }
  }
  return [...groups.values()];
}

/** What this browser keeps for one person: everything under their identifier, so that two people at one browser never meet. */
export function keptOnDevice(userId: string, storage: Pick<Storage, 'length' | 'key' | 'getItem'>): KeptItem[] {
  const prefix = `orch.${userId}.`;
  const found: KeptItem[] = [];
  for (let at = 0; at < storage.length; at += 1) {
    const key = storage.key(at);
    if (!key?.startsWith(prefix)) continue;
    // Two bytes to a character is what a browser stores them as, and close enough for a figure of "how much".
    found.push({ key, bytes: (storage.getItem(key) ?? '').length * 2 });
  }
  return found.sort((one, other) => one.key.localeCompare(other.key));
}

/** What a kept item is, in a word the page has: the layout of a kind of screen, or something else. */
export function kindOfKept(userId: string, key: string): { kind: 'layout'; screen: string } | { kind: 'comments' } | { kind: 'other'; name: string } {
  const rest = key.slice(`orch.${userId}.`.length);
  if (rest.startsWith('unsaved.document-comments.')) return { kind: 'comments' };
  const layout = /^workbench\.(.+)$/.exec(rest);
  return layout?.[1] === undefined ? { kind: 'other', name: rest } : { kind: 'layout', screen: layout[1] };
}

/** What this application keeps on a device besides layouts, by the first part of the name it is kept under. */
const KNOWN_KEPT = ['recents', 'org', 'help', 'tree', 'archived'] as const;

/** Which of those a kept name is, or `null` for a name this page does not know and shows as it is. */
export function knownKept(name: string): (typeof KNOWN_KEPT)[number] | null {
  const first = name.split('.')[0] ?? '';
  return KNOWN_KEPT.find((known) => known === first) ?? null;
}

/** What this browser keeps for the person, or nothing where the browser keeps nothing for the page. */
function readKept(userId: string | null): KeptItem[] {
  try {
    return userId === null ? [] : keptOnDevice(userId, localStorage);
  } catch {
    return [];
  }
}

function ThisDevice() {
  const userId = useStore(client.session, (state) => state.userId);
  return <ThisPersonDevice key={userId} userId={userId} />;
}
function ThisPersonDevice({ userId }: { userId: string | null }) {
  const { t } = useTranslation();
  const [items, setItems] = useState<KeptItem[]>(() => readKept(userId));
  const [forgetting, setForgetting] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const forget = (keys: readonly string[], all = false): boolean => {
    if (userId === null || client.session.getState().userId !== userId) return false;
    const removed: string[] = [];
    let failed = false;
    setError(null);
    try {
      for (const key of keys) {
        if (!key.startsWith(`orch.${userId}.`)) continue;
        localStorage.removeItem(key);
        removed.push(key);
      }
    } catch (failure) {
      failed = true;
      setError(failure);
    }
    forgetCommentDraftDeviceData(userId, all && !failed ? null : removed);
    forgetViewChoiceDeviceData(userId, all && !failed ? null : removed);
    setItems(readKept(userId));
    return !failed;
  };
  return (
    <Section id="interface-device" title={t('settings.interface.device.title')} lead={t('settings.interface.device.lead')}>
      {items.length === 0 ? (
        <p className="flex items-center gap-3 text-sm text-text-muted">
          <HardDrive className="size-4 shrink-0 text-text-subtle" aria-hidden />
          {t('settings.interface.device.none')}
        </p>
      ) : (
        <>
          <ul className={`${boxClasses} divide-y divide-border`}>
            {deviceKeptGroups(userId, items).map((item) => {
              const what = userId === null ? null : kindOfKept(userId, item.key);
              const known = what?.kind === 'other' ? knownKept(what.name) : null;
              const choice = item.choice;
              const scope = CHOICE_KINDS.find((kind) => kind === choice?.kind);
              const key = CHOICE_FIELDS.find((key) => key === choice?.key);
              const choiceName = choice === null ? null : scope === undefined ? t('craftChoiceNames.unknown')
                : scope === 'page' || key === undefined ? t(`craftChoiceNames.kinds.${scope}`)
                : t('craftChoiceNames.scope', { kind: t(`craftChoiceNames.kinds.${scope}`), choice: t(`craftChoiceNames.keys.${key}`) });
              const itemName = what?.kind === 'comments' ? t('settings.interface.device.commentDrafts') : what?.kind === 'other' ? what.name : item.key;
              return (
                <li key={item.id} className="flex items-center gap-3 px-4 py-2">
                  <HardDrive className="size-4 shrink-0 text-text-subtle" aria-hidden />
                  <span className="min-w-0 flex-1 text-base text-text">
                    {choiceName ?? (what?.kind === 'comments' ? itemName : what?.kind === 'layout'
                      ? t('settings.interface.device.layout', { screen: isScreenKind(what.screen) ? t(`settings.interface.device.screens.${what.screen}`) : what.screen })
                      : known !== null
                        ? t(`settings.interface.device.names.${known}`)
                        : t('settings.interface.device.other', { name: itemName }))}
                  </span>
                  <Button action={what?.kind === 'layout' ? 'resetLayout' : known === 'org' ? 'clearOrg' : 'clearChoices'} aria-label={what?.kind === 'layout' ? actionName('resetLayout', what.screen) : known === 'org' ? actionName('clearOrg') : actionName('clearChoices', choiceName ?? itemName)} size="sm" variant="ghost" onClick={() => { forget(item.keys); }}>
                    {what?.kind === 'layout' ? t('settings.interface.device.reset') : known === 'org' ? t('settings.interface.device.forget') : t('craftDevices.clear')}
                  </Button>
                </li>
              );
            })}
          </ul>
          <div>
            <Button variant="danger-ghost" onClick={() => setForgetting(true)}>
              {t('settings.interface.device.forgetAll')}
            </Button>
          </div>
        </>
      )}
      {error !== null && <ErrorAlert error={error} />}
      <ConfirmDialog
        open={forgetting}
        onOpenChange={setForgetting}
        title={t('settings.interface.device.forgetAllTitle')}
        confirmLabel={t('settings.interface.device.forgetAllConfirm')}
        danger
        onConfirm={() => {
          if (forget(readKept(userId).map((item) => item.key), true)) setForgetting(false);
        }}
      >
        {t('settings.interface.device.forgetAllBody')}
      </ConfirmDialog>
    </Section>
  );
}

export function InterfaceSettings() {
  const { t } = useTranslation();
  const state = useQuery(meStateQuery);
  const me = useQuery(meQuery);
  return (
    <PersonalFrame page="interface" lead={t('settings.interface.lead')}>
      {state.isPending ? (
        <SkeletonRows read={state} rows={4} />
      ) : state.isError ? (
        <ErrorState error={state.error} retrying={state.isRefetching} onRetry={() => void state.refetch()} />
      ) : (
        <div className="flex flex-col gap-6">
          {me.data !== undefined && (
            <Section id="interface-look" title={t('profile.sections.look')} lead={t('profile.sections.lookLead')}>
              <div className={`${boxClasses} p-4`}>
                <LookRows key={me.data.id} me={me.data} />
              </div>
            </Section>
          )}
          <Keys />
          <LinkSharing state={state.data} />
          <OpenLinks />
          <HiddenBars />
          <Reading />
          <Viewers state={state.data} />
          <Tutorials state={state.data} />
          <ThisDevice />
        </div>
      )}
    </PersonalFrame>
  );
}
