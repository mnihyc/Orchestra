/** Registry-driven scope editor.
 *
 * Every key of the registry that can be set at a place, by group, each with its sentence, the value in
 * force, where that value comes from, a way to set it here and to take it away again, a lock for
 * whoever makes the rules, and the lock of a place above where there is one. It is drawn from what the
 * server serves (the registry for the keys, the place's own list for the values) and names no key
 * of its own, so a key added in a later release appears in it without a change here. The same editor
 * serves an org at any level, a project and a topic; the system's settings and the read-only list of an
 * environment or a provider use it with another target.
 */
import { useViewLocation } from '@/features/workbench/pageContext';
import { useQuery } from '@tanstack/react-query';
import { Search, SlidersHorizontal } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { registryQuery, targetSettingsQuery, type SettingsTarget } from '@/api/settings';
import { DetailPages, DetailSections } from '@/features/resources/DetailSections';
import { Secrets } from '@/features/settings/Secrets';
import { SettingRow, type RowPlace } from '@/features/settings/SettingRow';
import { ReadOnlySettingsNotice } from '@/features/settings/ReadOnlyNotice';
import { localizedSetting } from '@/features/settings/settingText';
import { byGroup, keysAt, lockAbove, withGroup, type KeyDef } from '@/features/settings/settingsModel';
import type { ResolvedSetting } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { boxClasses, Section } from '@/shell/Page';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { ApiError } from '@/api/client';

export interface SettingsEditorProps {
  target: SettingsTarget;
  /** The scope the keys are valid at: `org`, `project`, `topic`, `system`, `environment` or `provider`. */
  scope: string;
  place: RowPlace;
  /** Keep to these keys: a section of a page that is one policy of the place. */
  only?: readonly string[];
  /** Resource pages browse one bounded section; the same registry rows own every edit. */
  compact?: boolean;
  /** A confirmed resource write, for the value table and other controls using this same read. */
  onChanged?: (next: ResolvedSetting) => void;
  /** Whether the secrets of the place are listed under the environment's group. */
  secrets?: boolean;
}

export function SettingsEditor(props: SettingsEditorProps) {
  const location = useViewLocation();
  const wantedKey = new URLSearchParams(location.searchStr).get('setting');
  // A newly selected URL key is a new editor session; ordinary query updates preserve its draft.
  return <EditorAtKey key={JSON.stringify([location.pathname,wantedKey])} {...props} wantedKey={wantedKey} />;
}

function EditorAtKey({ target, scope, place, only, compact = false, secrets = false, wantedKey, onChanged }: SettingsEditorProps & {wantedKey:string|null}) {
  const { t, i18n } = useTranslation();
  const location = useViewLocation();
  const body = useRef<HTMLDivElement>(null);
  const groupIds = useId();
  const [activeGroup, setActiveGroup] = useState('');
  const pendingGroup = useRef<string | null>(null);
  const reached = useRef<typeof location | null>(null);
  const registry = useQuery(registryQuery);
  const list = useQuery(targetSettingsQuery(target));
  const [q, setQ] = useState(wantedKey ?? '');
  const [onlyHere, setOnlyHere] = useState(false);
  const needle = useDebounced(q).trim().toLocaleLowerCase();
  useEffect(() => {
    if (wantedKey === null || reached.current === location) return;
    const row = [...(body.current?.querySelectorAll<HTMLElement>('[data-setting]') ?? [])].find((row) => row.dataset.setting === wantedKey);
    if (row === undefined) return;
    // The finder exits before the key's row receives the keyboard; the short outline matches other found rows.
    const previousFocus = document.activeElement;
    const focus = setTimeout(() => {
      if (!row.isConnected) return;
      reached.current = location;
      const active = document.activeElement;
      // A reader may already be editing, including in a portalled choice, before the finder settles.
      if (active !== row && (row.contains(active) || (active !== previousFocus && active !== document.body))) return;
      row.scrollIntoView({ block: 'center' });
      row.focus({ preventScroll: true });
    }, 200);
    row.classList.add('outline','outline-2','outline-focus','rounded-md');
    const clear = setTimeout(() => row.classList.remove('outline','outline-2','outline-focus','rounded-md'),2_000);
    return () => { clearTimeout(focus); clearTimeout(clear); row.classList.remove('outline','outline-2','outline-focus','rounded-md'); };
  },[location,wantedKey,needle,list.data,registry.data]);

  const language = i18n.resolvedLanguage;
  const defs = useMemo(() => keysAt((registry.data ?? []).map((def) => withGroup(localizedSetting(def, i18n, language))), scope), [registry.data, scope, i18n, language]);
  const byKey = useMemo(() => new Map((list.data ?? []).map((setting) => [setting.key, setting])), [list.data]);
  const shown = defs.filter((def) => {
    const setting = byKey.get(def.key);
    if (setting === undefined) return false;
    if (only !== undefined && !only.includes(def.key)) return false;
    if (onlyHere && setting.own === null) return false;
    return needle === '' || `${def.title} ${def.description} ${def.key} ${def.keywords.join(' ')}`.toLocaleLowerCase().includes(needle);
  });
  const groups = byGroup(shown);

  if (registry.data === undefined) return registry.isError ? <ErrorState compact error={registry.error} retrying={registry.isRefetching} onRetry={() => void registry.refetch()} /> : <SkeletonRows rows={6} />;
  const refused = list.error instanceof ApiError && [401, 403, 404].includes(list.error.status);
  if (list.data === undefined || refused) return list.isError ? <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} /> : <SkeletonRows rows={6} />;

  const filtered = needle !== '' || onlyHere;
  const contents = !compact && only === undefined && groups.length > 1;
  const readOnly = shown.some((def) => { const value = byKey.get(def.key); return value !== undefined && (!value.writable || lockAbove(value, target) !== null); });
  const editable = shown.some((def) => { const value = byKey.get(def.key); return value !== undefined && value.writable && lockAbove(value, target) === null; });
  const jump = (group: string) => {
    setActiveGroup(group);
    const section = document.getElementById(`${groupIds}-${group}`);
    section?.scrollIntoView({ block: 'start' });
    section?.focus({ preventScroll: true });
  };
  const renderRows = (keys: readonly KeyDef[], label: string) => <ul className={`${boxClasses} divide-y divide-border`} aria-label={label}>
    {keys.map((def) => { const setting = byKey.get(def.key); return setting === undefined ? null : <SettingRow key={setting.key} def={def} setting={setting} target={target} place={place} onChanged={onChanged} />; })}
  </ul>;
  const compactSections = groups.map(({ group, keys }) => ({ value: group, label: t(`settings.editor.groups.${group}`),
    panel: <DetailPages items={keys}>{(page) => renderRows(page, t(`settings.editor.groups.${group}`))}</DetailPages> }));
  return (
    // A container of its own: a row draws its title and its control side by side only where there is room, whichever pane the editor stands in.
    <div ref={body} className="@container flex flex-col gap-6">
      {registry.isError && <ErrorState compact error={registry.error} onRetry={() => void registry.refetch()} />}
      {list.isError && <ErrorState compact error={list.error} onRetry={() => void list.refetch()} />}
      {(compact || only === undefined) && defs.length > 0 && (
        <div className="flex flex-col gap-3">
          <div data-settings-filter className="flex flex-wrap items-center gap-x-4 gap-y-3">
            <Input
              label={t('settings.editor.search')}
              hideLabel
              type="search"
              placeholder={t('settings.editor.searchPlaceholder')}
              value={q}
              onChange={(event) => setQ(event.target.value)}
              fieldClassName="min-w-0 flex-1 basis-[min(100%,14rem)]"
              autoComplete="off"
            />
            <Switch label={t('settings.editor.onlyHere')} checked={onlyHere} onCheckedChange={setOnlyHere} className="shrink-0 items-center gap-3" />
          </div>
        </div>
      )}
      {readOnly && <ReadOnlySettingsNotice owner={place.owner} partial={editable} />}
      {groups.length === 0 ? (
        <EmptyState compact icon={filtered ? <Search aria-hidden /> : <SlidersHorizontal aria-hidden />} title={filtered ? t('settings.editor.noMatch') : t('settings.editor.none')} />
      ) : compact ? compactSections.length === 1 ? compactSections[0]?.panel : <DetailSections label={t('settings.editor.groupsNav')} sections={compactSections} /> : <div className="flex min-w-0 flex-col gap-4 @min-[48rem]:flex-row @min-[48rem]:items-start">
        {contents && <>
          <div className="bg-surface py-1 @min-[48rem]:hidden"><ChoiceLocator label={t('settings.editor.groupsNav')} hideLabel value="" placeholder={t('settings.compact.contents')}
            onValueChange={(group) => { pendingGroup.current = group; }} onCloseAutoFocus={(event) => {
              const group = pendingGroup.current;
              if (group === null) return;
              event.preventDefault(); pendingGroup.current = null; jump(group);
            }} options={groups.map(({ group }) => ({ value: group, name: t(`settings.editor.groups.${group}`) }))} /></div>
          {/* Reserve the shared chrome, scope heading and filter; a long contents list scrolls as navigation. Its current group
              is marked as every list marks one: a light ground, its weight and one bar, never a filled pill. */}
          <nav aria-label={t('settings.editor.groupsNav')} data-settings-contents data-scroll-allow="navigation"
            className="sticky top-3 hidden max-h-[calc(100dvh-14rem)] w-44 shrink-0 overflow-y-auto @min-[48rem]:block">
            <ul className="flex flex-col gap-1">{groups.map(({ group }) => <li key={group}><a href={`#${groupIds}-${group}`} aria-current={activeGroup === group ? 'location' : undefined}
              onClick={(event) => { event.preventDefault(); jump(group); }} className="focus-inset block rounded-md px-2 py-1 text-sm wrap-anywhere text-text-muted hover:bg-bg-muted hover:text-text aria-[current=location]:bg-accent-subtle aria-[current=location]:font-semibold aria-[current=location]:text-text aria-[current=location]:shadow-[inset_2px_0_0_var(--accent)]">{t(`settings.editor.groups.${group}`)}</a></li>)}</ul>
          </nav>
        </>}
        <div className="flex min-w-0 flex-1 flex-col gap-6">{
        groups.map(({ group, keys }) => {
          const rows = renderRows(keys, t(`settings.editor.groups.${group}`));
          // A screen that asks for a few keys by name has a heading of its own already: the group's would say the same twice.
          return only !== undefined ? (
            <div key={group}>{rows}</div>
          ) : (
            <div key={group} id={`${groupIds}-${group}`} tabIndex={-1} className="focus-inset min-w-0 scroll-mt-3 rounded-md"><Section id={`${groupIds}-${group}-title`} title={t(`settings.editor.groups.${group}`)}>
              {rows}
              {secrets && group === 'environment' && !filtered && <Secrets target={target} place={place} />}
            </Section></div>
          );
        })}</div>
      </div>}
      {secrets && only === undefined && !filtered && !groups.some(({ group }) => group === 'environment') && <Secrets target={target} place={place} />}
    </div>
  );
}
