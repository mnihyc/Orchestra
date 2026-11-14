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
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { registryQuery, targetSettingsQuery, type SettingsTarget } from '@/api/settings';
import { DetailPages } from '@/features/resources/DetailSections';
import { Secrets } from '@/features/settings/Secrets';
import { SettingRow, type RowPlace } from '@/features/settings/SettingRow';
import { ReadOnlySettingsNotice } from '@/features/settings/ReadOnlyNotice';
import { useDefaultsOpening } from '@/features/settings/defaultsOpening';
import { useDraftGuard } from '@/features/settings/drafts';
import { localizedSetting } from '@/features/settings/settingText';
import { byGroup, keysAt, lockAbove, withGroup, type KeyDef } from '@/features/settings/settingsModel';
import type { ResolvedSetting } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { boxClasses } from '@/shell/Page';
import { ScrollSections } from '@/shell/ScrollSections';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
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
  const opening = useDefaultsOpening(props.target.kind === 'node' ? props.target.id : null, undefined, props.only === undefined && props.compact !== true && props.secrets === true);
  if (opening.waiting) return <SkeletonRows read={opening.opening} rows={6} />;
  // A newly selected URL key is a new editor session; ordinary query updates preserve its draft.
  return <EditorAtKey key={JSON.stringify([location.pathname,wantedKey])} {...props} wantedKey={wantedKey} />;
}

function EditorAtKey({ target, scope, place, only, compact = false, secrets = false, wantedKey, onChanged }: SettingsEditorProps & {wantedKey:string|null}) {
  const { t, i18n } = useTranslation();
  const location = useViewLocation();
  const body = useRef<HTMLDivElement>(null);
  const reached = useRef<typeof location | null>(null);
  const registry = useQuery(registryQuery);
  const list = useQuery(targetSettingsQuery(target));
  const [q, setQ] = useState(wantedKey ?? '');
  const [onlyHere, setOnlyHere] = useState(false);
  // A row that holds an unsaved draft stays shown and mounted whatever the filter says, until its draft is saved or discarded.
  const [drafting, setDrafting] = useState<ReadonlySet<string>>(new Set());
  const keepDraft = useCallback((key: string, held: boolean) => setDrafting((before) => {
    if (before.has(key) === held) return before;
    const next = new Set(before);
    if (held) next.add(key); else next.delete(key);
    return next;
  }), []);
  useDraftGuard();
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
    if (drafting.has(def.key)) return true;
    if (onlyHere && setting.own === null) return false;
    return needle === '' || `${def.title} ${def.description} ${def.key} ${def.keywords.join(' ')}`.toLocaleLowerCase().includes(needle);
  });
  const groups = byGroup(shown);

  if (registry.data === undefined) return registry.isError ? <ErrorState compact error={registry.error} retrying={registry.isRefetching} onRetry={() => void registry.refetch()} /> : <SkeletonRows read={registry} rows={6} />;
  const refused = list.error instanceof ApiError && [401, 403, 404].includes(list.error.status);
  if (list.data === undefined || refused) return list.isError ? <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} /> : <SkeletonRows read={list} rows={6} />;

  const filtered = needle !== '' || onlyHere;
  const readOnly = shown.some((def) => { const value = byKey.get(def.key); return value !== undefined && (!value.writable || lockAbove(value, target) !== null); });
  const editable = shown.some((def) => { const value = byKey.get(def.key); return value !== undefined && value.writable && lockAbove(value, target) === null; });
  const renderRows = (keys: readonly KeyDef[], label: string) => <ul className={`${boxClasses} divide-y divide-border`} aria-label={label}>
    {keys.map((def) => { const setting = byKey.get(def.key); return setting === undefined ? null : <SettingRow key={setting.key} def={def} setting={setting} target={target} place={place} onChanged={onChanged} onDraft={keepDraft} />; })}
  </ul>;
  const groupName = (group: string) => t(`settings.editor.groups.${group}`);
  // Every group is on the page under its heading, with their list beside (a strip above where narrow) marking the one the page is at; nothing is chosen to be shown.
  // A resource page's group of more than a dozen rows is read a dozen at a time. A screen that asks for a few keys by name has a heading of its own already.
  const headed = compact ? byGroup(defs.filter((def) => byKey.has(def.key) && (only === undefined || only.includes(def.key)))).length > 1 : only === undefined;
  const groupBody = (group: string, keys: readonly KeyDef[]) => <>
    {compact ? <DetailPages items={keys}>{(page) => renderRows(page, groupName(group))}</DetailPages> : renderRows(keys, groupName(group))}
    {secrets && group === 'environment' && !filtered && <Secrets target={target} place={place} />}
  </>;
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
      ) : !headed ? <div className="flex min-w-0 flex-col gap-6">{groups.map(({ group, keys }) => <div key={group}>{groupBody(group, keys)}</div>)}</div>
        : <ScrollSections label={t('settings.editor.groupsNav')} level={compact ? 3 : 2} listFrom={compact ? 'page' : 'group'}
          sections={groups.map(({ group, keys }) => ({ id: group, title: groupName(group), body: groupBody(group, keys) }))} />}
      {secrets && only === undefined && !filtered && !groups.some(({ group }) => group === 'environment') && <Secrets target={target} place={place} />}
    </div>
  );
}
