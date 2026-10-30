/** A compact registry setting: direct edits, guarded drafts, and short links to whoever sets a read-only value. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Copy, Info, RotateCcw, Settings2, X } from 'lucide-react';
import { type ReactNode, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { environmentQuery } from '@/api/environments';
import { providerQuery } from '@/api/resources';
import { clearSetting, putSetting, targetSettingsKey, type SettingsTarget } from '@/api/settings';
import { WhereSet } from '@/features/help/WhereSet';
import { CallHelp } from '@/features/calls/Help';
import { SettingHint } from '@/features/help/SettingHint';
import { ORDERED_KEYS } from '@/features/settings/ValueControl';
import { SettingValue } from '@/features/settings/SettingValue';
import { isResourceSetting } from '@/features/settings/ResourceChoice';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { emptySettingText, emptySettingValue, SettingSummary } from '@/features/settings/SettingSummary';
import { SettingReachLine } from '@/features/settings/Reach';
import { editableValue } from '@/features/settings/settingParts';
import { type KeyDef, availableShape, honoursLocks, lockAbove, provenanceOf, sameValue, words } from '@/features/settings/settingsModel';
import { settingShape } from '@/features/settings/settingText';
import { memoryBelowUse, runtimeEffect } from '@/features/settings/runtimeEffects';
import type { Access, ResolvedSetting, UserRef } from '@/generated/api-types';
import { formatBytes } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { HelpHint } from '@/ui/HelpHint';
import { SettingGuideLink, settingGuide } from '@/features/help/SettingGuideLink';
import { LockBadge } from '@/ui/LockBadge';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { SettingEditor } from '@/ui/SettingEditor';
import { SettingChanges } from '@/ui/SettingChanges';
import { SettingRow as SettingRowLayout, type SettingRowProps as SettingRowLayoutProps } from '@/ui/SettingRow';
import { Tooltip } from '@/ui/Tooltip';
import { copyText } from '@/ui/clipboard';

/** What a row needs to know of the place it is shown at. */
export interface RowPlace {
  name: string;
  owner: Pick<UserRef, 'display_name'> | null;
  access: Pick<Access, 'permissions'> | null;
  /** The kind of node it is, where it is one: `org`, `project` or `topic`. */
  kind: string | null;
}

/** Says a word of the registry in the reader's language, or as it is written where the catalogue has no word for it. */
export function useSay(): (word: string) => string {
  const { i18n: instance } = useTranslation();
  return (word) => {
    const key = `settings.editor.words.${word}`;
    return instance.exists(key) ? instance.t(key, { defaultValue: words(word) }) : words(word);
  };
}

export interface SettingRowProps {
  def: KeyDef;
  setting: ResolvedSetting;
  target: SettingsTarget;
  place: RowPlace;
  /** The row after the server changed it. */
  onChanged?: (next: ResolvedSetting) => void;
  /** Compatibility: editing authority is now explained once by the containing collection. */
  quiet?: boolean;
  /** A resource can state its declared lifecycle/apply constraint in this same row. */
  effectNote?: ReactNode;
}

interface SavedSetting { before: ResolvedSetting['own']; revision: number | null }
interface WriteChange { value: unknown; locked?: boolean; before: ResolvedSetting['own']; guard: number | null }

export function SettingRow({ def, setting: incoming, target, place, onChanged, effectNote }: SettingRowProps) {
  const { t, i18n: instance } = useTranslation();
  const client = useQueryClient();
  const locale = instance.resolvedLanguage ?? 'en';
  const say = useSay();
  const [setting, setSetting] = useState(incoming);
  const [seen, setSeen] = useState(incoming);
  if (seen !== incoming) { setSeen(incoming); setSetting(incoming); }
  const latest = useRef(incoming);
  useLayoutEffect(() => { latest.current = incoming; }, [incoming]);
  const concernsRuntime = def.key === 'env.resources' || def.key === 'runtime.resources' || def.key === 'env.mounts';
  const environment = useQuery({ ...environmentQuery(target.kind === 'environment' ? target.id : ''), enabled: concernsRuntime && target.kind === 'environment' });
  const provider = useQuery({ ...providerQuery(target.kind === 'provider' ? target.id : ''), enabled: concernsRuntime && target.kind === 'provider' });
  const runtime = target.kind === 'environment' ? environment.data?.runtime_changes : target.kind === 'provider' ? provider.data?.runtime_changes : undefined;
  const effect = (target.kind === 'environment' || target.kind === 'provider') ? runtimeEffect(def.key, runtime) : null;
  const shape = useMemo(() => availableShape(settingShape(def), setting.available_values), [def, setting.available_values]);
  // A discrete choice commits immediately; free input commits on leaving its field/form or Enter.
  const joining = def.merge !== 'replace';
  const instant = !joining && (shape.type === 'bool' || shape.type === 'choice' || def.key === 'agent.default_provider_id' || def.key === 'calls.rules' || isResourceSetting(def.key));
  const inForce = editableValue(def, setting, shape);
  // A remote ancestor or own change must not discard what this person is still writing.
  const [draft, setDraft] = useState<unknown>(inForce);
  const [base, setBase] = useState<unknown>(inForce);
  const [baseRevision, setBaseRevision] = useState<number | null>(setting.own?.revision ?? null);
  const [baseOwn, setBaseOwn] = useState(setting.own);
  const [observed, setObserved] = useState({ value: inForce, revision: setting.own?.revision ?? null });
  const [touched, setTouched] = useState(false);
  const [editorOpen, setEditorOpen] = useState(false);
  const [invalid, setInvalid] = useState(false);
  const [saved, setSaved] = useState<SavedSetting | null>(null);
  const [feedback, setFeedback] = useState<'saved' | 'undone' | null>(null);
  // Cancel draws the control anew, so that what it holds of what was typed is put back as well.
  const [attempt, setAttempt] = useState(0);
  const ownChanged = !sameValue(base, inForce) || baseRevision !== (setting.own?.revision ?? null);
  const edited = touched || !sameValue(draft, base);
  const dirty = edited;
  const incomingChanged = !sameValue(observed.value, inForce) || observed.revision !== (setting.own?.revision ?? null);
  if (incomingChanged) {
    // A confirmed write can render before its query notification. Only a new served row may
    // replace an untouched draft; the old prop must not undo the just-confirmed local value.
    setObserved({ value: inForce, revision: setting.own?.revision ?? null });
    if (!edited) {
      setBase(inForce);
      setDraft(inForce);
      setBaseRevision(setting.own?.revision ?? null);
      setBaseOwn(setting.own);
      setSaved(null);
      setFeedback(null);
      setInvalid(false);
      setAttempt((count) => count + 1);
    }
  }
  // The details of the key are shown when the row's menu opens them.
  const [detailed, setDetailed] = useState(false);
  const form = useRef<HTMLFormElement>(null);
  const refocus = useRef(false);
  const returnControl = useRef<HTMLElement | null>(null);
  const confirmed = (accepted: ResolvedSetting, before: ResolvedSetting['own']) => {
    // A live row may arrive after this write was accepted but before its HTTP answer. The
    // accepted revision still fences Undo; it must never roll a newer own value back in the UI.
    const cached = client.getQueryData<ResolvedSetting[]>(targetSettingsKey(target))?.find((row) => row.key === def.key);
    const next = [latest.current, cached].reduce<ResolvedSetting>((held, candidate) => candidate !== undefined
      && candidate.own !== null && candidate.own.revision !== before?.revision && candidate.own.revision > (held.own?.revision ?? 0) ? candidate : held, accepted);
    const value = editableValue(def, next, shape);
    setSetting(next);
    setDraft(value);
    setBase(value);
    setBaseRevision(next.own?.revision ?? null);
    setBaseOwn(next.own);
    setObserved({ value, revision: next.own?.revision ?? null });
    setTouched(false);
    setEditorOpen(false);
    setAttempt((count) => count + 1);
    setInvalid(false);
    client.setQueryData<ResolvedSetting[]>(targetSettingsKey(target), (before) => before?.map((row) => row.key === next.key ? next : row));
    onChanged?.(next);
  };
  const reread = (error: unknown) => {
    if (error instanceof ApiError && error.is('conflict.stale_revision')) void client.invalidateQueries({ queryKey: targetSettingsKey(target), exact: true });
  };

  const write = useMutation({
    mutationFn: (change: WriteChange) => putSetting(target, def.key, { value: change.value, ...(change.locked === undefined ? {} : { locked: change.locked }), expectedRevision: change.guard }),
    onSuccess: (next, change) => { confirmed(next, change.before); setSaved({ before: change.before, revision: next.own?.revision ?? null }); setFeedback('saved'); },
    onError: reread,
  });
  const reset = useMutation({ mutationFn: (change: { before: ResolvedSetting['own']; guard: number | null }) => clearSetting(target, def.key, change.guard),
    onSuccess: (next, change) => { confirmed(next, change.before); setSaved({ before: change.before, revision: next.own?.revision ?? null }); setFeedback('saved'); }, onError: reread,
  });
  const undo = useMutation({ mutationFn: (change: SavedSetting) => change.before === null ? clearSetting(target, def.key, change.revision)
    : putSetting(target, def.key, { value: change.before.value, locked: change.before.locked, expectedRevision: change.revision }),
    onSuccess: (next, change) => { confirmed(next, setting.own?.revision === change.revision ? setting.own : null); setSaved(null); setFeedback('undone'); }, onError: reread,
  });
  const writeValue = (value: unknown, locked?: boolean, keepFocus = true) => {
    refocus.current = keepFocus;
    returnControl.current = form.current?.closest('[data-setting-row]')?.querySelector<HTMLElement>('[data-setting-editor-trigger]') ?? null;
    reset.reset(); undo.reset(); write.mutate({ value, ...(locked === undefined ? {} : { locked }), before: baseOwn, guard: baseRevision });
  };

  const provenance = provenanceOf(setting, target);
  const higherLock = lockAbove(setting, target);
  const lockedAbove = provenance.kind === 'locked';
  const mayLock = target.kind === 'system' || holds(place.access, 'settings.manage');
  const writable = setting.writable && !lockedAbove;
  const busy = write.isPending || reset.isPending || undo.isPending;
  const shown = write.isPending && instant ? write.variables.value : draft;
  const error = write.error ?? reset.error ?? undo.error;
  const stale = error instanceof ApiError && error.is('conflict.stale_revision');
  const faults = error instanceof ApiError ? error.faults() : [];
  const fault = faults.find((entry) => entry.path === 'value' || entry.path === '')?.message ?? faults[0]?.message ?? (invalid ? t('settings.compact.invalidValue') : undefined);

  const lockable = honoursLocks(def) && (target.kind === 'node' || target.kind === 'system');
  const locked = setting.own?.locked === true;

  const complex = joining || !['bool', 'int', 'choice', 'text', 'id', 'time'].includes(shape.type) || shape.type === 'text' && (shape.maxLength ?? 0) > 200;
  useLayoutEffect(() => {
    if (busy || !refocus.current) return;
    refocus.current = false;
    if (document.activeElement !== document.body && form.current?.contains(document.activeElement) !== true) return;
    if (complex) returnControl.current?.focus();
    else form.current?.querySelector<HTMLElement>('input:not([type="hidden"]):not([disabled]), textarea:not([disabled]), [role="switch"], [role="combobox"]')?.focus();
  }, [attempt, busy, complex]);
  const commit = (keepFocus = true) => {
    if (!dirty || busy || !writable || instant || stale || ownChanged) return;
    if (form.current?.checkValidity() !== true) { setInvalid(true); return; }
    if (sameValue(draft, base)) { setTouched(false); setInvalid(false); setAttempt((count) => count + 1); return; }
    writeValue(draft, undefined, keepFocus);
  };
  // An explicit Apply uses the control's new value before the row has rendered that draft.
  const applyValue = (value: unknown) => {
    setDraft(value); setTouched(true); setInvalid(false);
    if (busy || !writable || stale || ownChanged) return;
    if (form.current?.checkValidity() !== true) { setInvalid(true); return; }
    if (sameValue(value, base)) { setTouched(false); setAttempt((count) => count + 1); setEditorOpen(false); return; }
    writeValue(value);
  };

  const copyKey = () => copyText(def.key, t('settings.editor.keyCopied', { key: def.key }));
  const resetEntry: MenuEntry | null = setting.own !== null && writable
    ? { id: 'reset', label: t('settings.editor.reset'), icon: <RotateCcw aria-hidden />, disabled: busy ? t('settings.editor.saving') : edited ? t('settings.compact.finishDraft') : false,
      onSelect: () => { write.reset(); undo.reset(); reset.mutate({ before: baseOwn, guard: baseRevision }); } } : null;
  const showLock = lockable && mayLock && !lockedAbove;
  const mayToggleLock = writable && !busy && !edited && setting.value !== null;
  const lockEntries: MenuEntry[] = !showLock ? [] : mayToggleLock
    ? [{ type: 'check', id: 'lock', name: t('settings.editor.lock'), checked: locked, onCheckedChange: (next) => writeValue(setting.own?.value ?? (joining ? draft : setting.value), next) }]
    : [{ id: 'lock', name: t('settings.editor.lock'), disabled: t(!writable ? 'settings.compact.readOnly' : busy ? 'settings.editor.saving' : setting.value === null ? 'settings.compact.needsValue' : 'settings.compact.finishDraft'), onSelect: () => undefined }];
  const entries: MenuEntry[] = [
    { id: 'details', label: detailed ? t('settings.editor.hideDetails') : t('settings.editor.showDetails'), icon: <Info aria-hidden />, onSelect: () => setDetailed((was) => !was) },
    ...(resetEntry === null ? [] : [resetEntry]),
    { id: 'copy', label: t('settings.editor.copyKey'), icon: <Copy aria-hidden />, onSelect: () => void copyKey() },
    ...lockEntries,
  ];

  // A lock can ignore a stored own value: readers see the effective answer, never that ignored draft.
  const visibleValue = writable ? shown : setting.value;
  const showsDefault = writable && provenance.kind === 'default' && !edited;
  const emptyDefault = showsDefault && emptySettingValue(shown);
  const reason = higherLock !== null ? <SourceSettingLink locked source={higherLock} def={def} target={target} fallback={place.name} />
    : provenance.kind === 'default' ? showsDefault ? undefined : <span>{t('settings.editor.from.default')}</span>
      : provenance.kind === 'here' ? <span>{t('settings.compact.setOn', { place: place.name })}</span>
      : <SourceSettingLink inherited={provenance.kind === 'inherited'} source={setting.source} def={def} target={target} fallback={place.name} />;

  const controls = <form ref={form} className="flex min-w-0 max-w-full flex-wrap items-center gap-2" aria-busy={busy || undefined}
    onBlur={(event) => { if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) {
      commit(event.relatedTarget instanceof Node && event.currentTarget.closest('[data-setting-row]')?.contains(event.relatedTarget) === true);
    } }}
    onKeyDown={(event) => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); event.stopPropagation(); commit(); } }}
    onSubmit={(event) => { event.preventDefault(); commit(); }}>
    {showsDefault && !emptyDefault && <span className="text-xs text-text-muted">{t('settings.compact.defaultPrefix')}</span>}
    <SettingValue key={attempt} shape={shape} def={def} setting={setting} target={target} display={def.display} placeholder={emptyDefault ? emptySettingText(def.key, true, t) : undefined}
      value={shown} label={def.title} disabled={busy} error={fault} onEdited={() => { setTouched(true); setInvalid(false); }} say={say}
      ordered={ORDERED_KEYS.has(def.key)} onApply={applyValue} onChange={(next) => { setDraft(next); setTouched(true); setInvalid(false); if (instant && writable) writeValue(next); }} />
    {dirty && write.isError && !stale && !ownChanged && <Button size="sm" pending={busy} onClick={() => { if (instant) writeValue(draft); else commit(); }}>{t('common.tryAgain')}</Button>}
    {dirty && !instant && <>
      <Button size="sm" icon={<X aria-hidden />} disabled={busy} onClick={() => { setDraft(inForce); setBase(inForce); setBaseRevision(setting.own?.revision ?? null); setBaseOwn(setting.own); setTouched(false); setInvalid(false); setAttempt((count) => count + 1); setEditorOpen(false); write.reset(); }}>{t('common.cancel')}</Button></>}
  </form>;
  const showsMemory = (def.key === 'env.resources' || def.key === 'runtime.resources') && (target.kind === 'environment' || target.kind === 'provider');
  const applicability = typeof setting.automation_reason === 'string' ? setting.automation_reason.startsWith('calls.') ? t(setting.automation_reason, { defaultValue: t('calls.servicePaused') }) : setting.automation_reason : typeof setting.not_applicable_reason === 'string' ? t('inspector.parameters.subagentsUnsupported') : undefined;
  const example = shape.type !== 'bool' && shape.type !== 'choice' && !(shape.type === 'list' && shape.item.type === 'choice') && def.example !== null && def.example !== undefined && def.example !== '' && !sameValue(def.example, setting.value)
    ? <><p>{t('settings.compact.example')}</p><SettingSummary def={def} value={def.example} example /></> : null;
  const range = shape.type === 'int' && (shape.min !== null || shape.max !== null) ? shape.min !== null && shape.max !== null ? t('settings.editor.between', { min: shape.min, max: shape.max })
    : shape.min !== null ? t('settings.editor.atLeast', { min: shape.min }) : t('settings.editor.atMost', { max: shape.max ?? 0 }) : null;
  const semantic = def.key.startsWith('search.');
  const extraHelp = semantic || joining || example !== null || range !== null || applicability !== undefined || settingGuide(def) !== null;
  const supplementaryHelp = <>
    {semantic && <p>{def.description}</p>}
    {joining && <p>{t(`settings.editor.detail.merges.${def.merge}`)}</p>}
    {range !== null && <p>{range}</p>}
    {example}
    {applicability !== undefined && <p>{applicability}</p>}
    <SettingGuideLink def={def} target={target} />
  </>;
  const undoBlocked = busy || dirty || saved?.revision !== (setting.own?.revision ?? null) || undo.isError || !writable;
  const callSection = def.key === 'calls.rules' ? 'rules' : def.key === 'calls.return' ? 'candidates' : def.key.startsWith('calls.') ? 'guard' : null;

  const row: Omit<SettingRowLayoutProps, 'value'> = {
      name: def.title, description: semantic ? undefined : def.description, settingKey: def.key, readOnly: !writable, reason,
      reach: (target.kind === 'node' || target.kind === 'system') ? <SettingReachLine target={target} def={def} name={def.title} snapshot={JSON.stringify([setting.value, setting.own, setting.locked_by, setting.source, setting.parts])} /> : undefined,
      help: def.key === 'retention.agent_history_idle_days' || semantic ? <SettingHint def={def} setting={setting} target={target} /> : callSection !== null ? <CallHelp section={callSection} term={def.title} what={<><p>{t(`calls.help.${callSection}.what`)}</p>{writable && extraHelp && supplementaryHelp}</>} /> : writable && extraHelp ? <HelpHint term={def.title} what={supplementaryHelp} /> : undefined,
      actions: <>{locked && <LockBadge label={t('settings.editor.lockedHere')} />}
        <Menu label={t('settings.editor.menuOf', { name: def.title })} align="end" entries={entries} trigger={<IconButton size="sm" label={t('settings.editor.menuOf', { name: def.title })} icon={<Settings2 aria-hidden />} />} /></>,
      details: detailed ? (
          <div aria-label={t('settings.editor.detailsOf', { name: def.title })} className="mt-1 flex flex-col gap-3 text-sm">
            <WhereSet additionalOnly values setting={setting} def={def} target={target} />
          </div>
      ) : undefined,
      status: setting.automation_reason?.startsWith('calls.') === true || busy || !dirty && feedback !== null || effectNote !== undefined && effectNote !== null || effect !== null || showsMemory || invalid || ownChanged && edited || error !== null || !editorOpen && complex && edited ? <>
        {setting.automation_reason?.startsWith('calls.') === true && <p role="status" className="flex flex-wrap items-center gap-1 text-sm text-text-muted">{t(setting.automation_reason, { defaultValue: t('calls.servicePaused') })}<CallHelp section="services" /></p>}
        {busy && <p className="text-sm text-text-muted">{t('settings.editor.saving')}</p>}
        {!busy && !dirty && feedback !== null && <div role="status" className="flex items-center gap-2 text-xs text-text-muted"><span>{t(feedback === 'saved' ? 'settings.compact.saved' : 'settings.compact.undone')}</span>
          {saved !== null && <Tooltip content={undoBlocked ? t('settings.compact.undoStale') : undefined}><Button size="sm" variant="ghost" icon={<RotateCcw aria-hidden />} blocked={undoBlocked} aria-label={t('settings.compact.undoNamed', { name: def.title })} onClick={() => { write.reset(); reset.reset(); undo.mutate(saved); }}>{t('settings.compact.undo')}</Button></Tooltip>}</div>}
        {!editorOpen && complex && edited && !busy && !invalid && <p role="status" className="text-xs text-text-muted">{t('settings.compact.draft')}</p>}
        {invalid && complex && !editorOpen && <p role="alert" className="text-xs text-danger-text">{t('settings.compact.invalidValue')}</p>}
        {effectNote !== undefined ? effectNote : effect !== null && <p className="text-sm text-text-muted">{t('runtimeChanges.takesEffect')}: {t(`runtimeChanges.${effect}`)}</p>}
        {effectNote === undefined && effect === 'pendingResources' && <p role="status" className="text-sm text-warning-text">{runtime?.pending_reason}</p>}
        {showsMemory && (
          <p role="status" className="text-sm text-text-muted">
            {runtime?.memory_used_bytes === null || runtime?.memory_used_bytes === undefined ? t('runtimeChanges.memoryUnknown') : t('runtimeChanges.memoryUsed', { size: formatBytes(runtime.memory_used_bytes, locale) })}
            {memoryBelowUse(visibleValue, runtime?.memory_used_bytes) && <span className="block text-warning-text">{t('runtimeChanges.memoryLower')}</span>}
          </p>
        )}
        {(stale || ownChanged && edited) && <InlineAlert tone="warning"><p>{t(dirty ? 'settings.compact.changedReason' : 'settings.compact.undoStale')}</p>{dirty ? <SettingChanges rows={[{ name: def.title, current: inForce, draft }]} disabled={!ownChanged || busy || !writable} reason={invalid ? t('settings.compact.invalidValue') : undefined} onKeep={() => {
          refocus.current = true;
          returnControl.current = document.activeElement?.closest('[data-setting-row]')?.querySelector<HTMLElement>('[data-setting-editor-trigger]') ?? null;
          setBase(inForce); setBaseRevision(setting.own?.revision ?? null); setBaseOwn(setting.own); setTouched(!sameValue(draft, inForce)); setSaved(null); write.reset(); reset.reset(); undo.reset();
          if (!sameValue(draft, inForce)) write.mutate({ value: draft, before: setting.own, guard: setting.own?.revision ?? null });
          else setAttempt((count) => count + 1);
        }} /> : <Button size="sm" onClick={() => { setSaved(null); setFeedback(null); write.reset(); reset.reset(); undo.reset(); }}>{t('common.dismiss')}</Button>}</InlineAlert>}
        {!editorOpen && error !== null && !stale && fault === undefined && <ErrorAlert error={error} />}
      </> : undefined,
  };
  const summary = emptySettingValue(setting.value) ? <span>{emptySettingText(def.key, showsDefault, t)}</span>
    : <>{showsDefault && <span className="mr-1 text-xs text-text-muted">{t('settings.compact.defaultPrefix')}</span>}<SettingSummary inControl def={def} value={setting.value} /></>;
  return complex && writable ? <SettingEditor {...row} placement="inline" summary={summary} label={t('settings.compact.editValue', { name: def.title })}
    open={editorOpen} onOpenChange={setEditorOpen} disabled={busy}>
    {controls}
    {error !== null && <ErrorAlert error={error} />}
  </SettingEditor> : <SettingRowLayout {...row} value={writable ? controls : emptySettingValue(setting.value) ? <span>{emptySettingText(def.key, false, t)}</span> : <SettingSummary def={def} value={setting.value} />} />;
}
