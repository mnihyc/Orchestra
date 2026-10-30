/** The list of keys, which is also where they are changed: every action of the application with the keys it has now, and the few keys that the controls themselves handle.
 *
 * The actions the screens shown now ask for come first, as "Here"; the others work on other screens and are
 * listed after them, so that their keys can be changed from anywhere. Change takes the next keys pressed (one
 * chord, or two in a row as in G I), and refuses keys another action has or begins with, keys the browser or
 * the system keeps, and keys a control moves with; Esc gives up. Remove leaves the action without keys, and
 * Reset gives it back the keys it shipped with. A choice is kept with the person (`keys` of their state) and
 * acts once it is saved; a save that fails keeps the keys before acting and offers to try again. The keys that
 * a control handles by itself (a tab picked up with the space bar, the menu key) are listed once and are not
 * changed here.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { StateLine } from '@/ui/StateLine';
import { shortcutMatches } from '@/features/help/shortcutSearch';
import { EmptyState } from '@/ui/EmptyState';
import { Input } from '@/ui/Input';
import { Table, type Column } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';

import { meStateKey, meStateQuery, setMeState } from '@/api/personal';
import type { TranslationKey } from '@/i18n';
import { type Chord, formatShortcut, isApplePlatform, keyRows, SEQUENCE_WINDOW_MS } from '@/lib/shortcuts';
import {
  chordOfEvent,
  currentKeys,
  isKeyAction,
  isSingleKey,
  KEY_ACTION_IDS,
  keyAction,
  type KeyActionId,
  keptChoices,
  type KeysRefused,
  normalShortcut,
  refusalOf,
} from '@/shell/keyActions';
import { useAskedActions, useBindings, useKeyChoices } from '@/shell/keys';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Kbd } from '@/ui/Kbd';

/** The keys that work inside a control and are bound by it, not by the shell. */
export const CONTROL_KEYS: readonly { keys: string; what: TranslationKey }[] = [
  { keys: 'Space', what: 'shell.shortcuts.carryTab' },
  { keys: 'Shift+F10', what: 'shell.shortcuts.contextMenu' },
];

const WHERE_COLUMN = 'where';

/** An action's name in the list: its own, where several share what they do, else what it does. */
export function keyActionName(t: TFunction, id: KeyActionId): string {
  const action = keyAction(id);
  if (action.name === undefined) return t(action.what);
  return action.tab === undefined ? t(action.name) : t(action.name, { n: action.tab });
}

/** A change to the person's keys: the single-key switch, or one action's keys (`null`: none; `undefined`: as shipped). */
export interface KeysChange {
  single_key?: boolean;
  binding?: { id: KeyActionId; to: string | null | undefined };
}

/** Saves a change to the person's keys onto what the server holds now, keeping every entry this build does not know. */
export function useSaveKeys() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ['personal-keys'],
    scope: { id: 'personal-keys' },
    mutationFn: async (change: KeysChange) => {
      const state = client.getQueryData<Record<string, unknown>>(meStateKey) ?? await client.query(meStateQuery);
      const next = keptChoices(state.keys, change);
      await setMeState('keys', next);
      return next;
    },
    onSuccess: (next) => {
      client.setQueryData<Record<string, unknown>>(meStateKey, (state) => ({ ...state, keys: next }));
    },
  });
}

function refusalText(t: TFunction, keys: string, why: KeysRefused): string {
  switch (why.kind) {
    case 'taken':
      return t('shell.keyChoice.taken', { keys, other: keyActionName(t, why.other) });
    case 'prefix':
      return t('shell.keyChoice.prefix', { keys, other: keyActionName(t, why.other) });
    case 'reserved':
      return t('shell.keyChoice.reserved', { keys });
    case 'unusable':
      return t('shell.keyChoice.unusable', { keys });
  }
}

/** The keys a change that was not saved proposed, in words. */
function proposedText(t: TFunction, change: KeysChange | undefined, shipped: string): string {
  const to = change?.binding?.to;
  if (to === null) return t('shell.keyChoice.none');
  return to ?? shipped;
}

/** Takes the next keys pressed: one chord, or a second within the time a sequence waits. Esc gives up. */
function Capture({ name, onKeys, onCancel }: { name: string; onKeys: (keys: string) => void; onCancel: (restoreFocus: boolean) => void }) {
  const { t } = useTranslation();
  const box = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [chords, setChords] = useState<Chord[]>([]);
  useEffect(() => {
    box.current?.focus();
    return () => {
      if (timer.current !== null) clearTimeout(timer.current);
    };
  }, []);
  const finish = (taken: Chord[]) => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
    onKeys(formatShortcut(taken));
  };
  return (
    <div
      ref={box}
      tabIndex={0}
      role="group"
      aria-label={t('shell.keyChoice.pressing', { action: name })}
      data-keys="own"
      className="flex min-h-8 min-w-40 items-center gap-1 rounded-md border border-accent px-2 text-sm text-text-muted focus-visible:outline-2 focus-visible:outline-accent"
      onBlur={() => {
        if (timer.current !== null) clearTimeout(timer.current);
        timer.current = null;
        onCancel(false);
      }}
      onKeyDown={(event) => {
        // Nothing else hears these keys: they are being chosen, not used.
        event.preventDefault();
        event.stopPropagation();
        if (event.key === 'Escape') {
          if (timer.current !== null) clearTimeout(timer.current);
          timer.current = null;
          onCancel(true);
          return;
        }
        const chord = chordOfEvent(event.nativeEvent, isApplePlatform());
        if (chord === null) return;
        const taken = [...chords, chord];
        if (taken.length >= 2) {
          finish(taken);
          return;
        }
        setChords(taken);
        // A second key may follow, as in G I; without one, the first is the keys.
        timer.current = setTimeout(() => finish(taken), SEQUENCE_WINDOW_MS);
      }}
    >
      {chords.length === 0 ? <span aria-live="polite">{t('shell.keyChoice.press')}</span> : <Kbd shortcut={formatShortcut(chords)} />}
    </div>
  );
}

function ActionRow({ id }: { id: KeyActionId }) {
  const { t } = useTranslation();
  const choices = useKeyChoices();
  const save = useSaveKeys();
  const [capturing, setCapturing] = useState(false);
  const changeButton = useRef<HTMLButtonElement>(null);
  const wasCapturing = useRef(false);
  const returnFocus = useRef(false);
  useEffect(() => {
    if (wasCapturing.current && !capturing && returnFocus.current) { changeButton.current?.focus(); returnFocus.current = false; }
    wasCapturing.current = capturing;
  }, [capturing]);
  const [refused, setRefused] = useState<{ keys: string; why: KeysRefused } | null>(null);
  const name = keyActionName(t, id);
  const shipped = keyAction(id).shipped;
  const keys = currentKeys(id, choices);
  const own = choices.bindings[id] !== undefined;
  const off = keys !== null && !choices.single_key && isSingleKey(keys);

  const choose = (pressed: string) => {
    returnFocus.current = true;
    setCapturing(false);
    const why = refusalOf(id, pressed, choices, isApplePlatform());
    if (why !== null) {
      setRefused({ keys: pressed, why });
      return;
    }
    const written = normalShortcut(pressed);
    // The keys it shipped with are no choice of the person's: they are kept as shipped.
    save.mutate({ binding: { id, to: written === normalShortcut(shipped) ? undefined : written } });
  };

  return (
    <div data-key-action={id} aria-description={off ? t('shell.keyChoice.off') : undefined} className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
      <span className="flex min-w-24 shrink-0 items-center gap-1 text-text-subtle">
        {keys === null ? <span className="text-sm">{t('shell.keyChoice.none')}</span> : <Kbd shortcut={keys} />}
        {off && <Tooltip content={t('shell.keyChoice.off')}><span className="text-sm">{t('shell.keyChoice.offState')}</span></Tooltip>}
      </span>
      {capturing ? (
        <Capture name={name} onKeys={choose} onCancel={(restore) => { returnFocus.current = restore; setCapturing(false); }} />
      ) : (
        <span data-key-actions className="flex shrink-0 items-center gap-1 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 [@media(hover:none)]:opacity-100">
          <Button
            ref={changeButton}
            size="sm"
            variant="ghost"
            aria-label={t('shell.keyChoice.changeOf', { action: name })}
            pending={save.isPending}
            onClick={() => {
              setRefused(null);
              setCapturing(true);
            }}
          >
            {t('shell.keyChoice.change')}
          </Button>
          {keys !== null && (
            <Button size="sm" variant="ghost" aria-label={t('shell.keyChoice.removeOf', { action: name })} pending={save.isPending} onClick={() => save.mutate({ binding: { id, to: null } })}>
              {t('shell.keyChoice.remove')}
            </Button>
          )}
          {own && (
            <Button size="sm" variant="ghost" aria-label={t('shell.keyChoice.resetOf', { action: name, keys: shipped })} pending={save.isPending} onClick={() => save.mutate({ binding: { id, to: undefined } })}>
              {t('shell.keyChoice.reset')}
            </Button>
          )}
        </span>
      )}
      {refused !== null && (
        <StateLine role="alert" tone="danger" compact className="w-full max-w-sm whitespace-normal" message={refusalText(t, refused.keys, refused.why)} />
      )}
      {save.error !== null && (
        <div className="flex w-full max-w-sm flex-col gap-1 whitespace-normal">
          {/* What was proposed stays in view; the keys before act until it is saved. */}
          <p className="text-sm text-text-muted">{t('shell.keyChoice.failed', { keys: proposedText(t, save.variables, shipped) })}</p>
          <ErrorAlert
            error={save.error}
            action={
              <Button size="sm" onClick={() => save.mutate(save.variables)}>
                {t('shell.keyChoice.retry')}
              </Button>
            }
          />
        </div>
      )}
    </div>
  );
}

export function KeyList() {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const search = useRef<HTMLInputElement>(null);
  const choices = useKeyChoices();
  const asked = new Set(useAskedActions());
  // A key a screen binds that is none of the application's actions is listed as it is, and cannot be changed.
  const others = keyRows(useBindings().filter((binding) => !isKeyAction(binding.id)));
  interface Row { id: string; action?: KeyActionId; name: string; keys: string[]; where: string }
  const here = t('shell.keyChoice.here');
  const rows: Row[] = [
    ...KEY_ACTION_IDS.map((id): Row => {
      const keys = currentKeys(id, choices);
      return { id, action: id, name: keyActionName(t, id), keys: keys === null ? [] : [keys], where: t(asked.has(id) ? 'shell.keyChoice.here' : 'shell.keyChoice.elsewhere') };
    }),
    ...others.map((row): Row => ({ id: `external:${row.what}`, name: t(row.what), keys: [row.first, ...(row.last === null ? [] : [row.last])], where: here })),
    ...CONTROL_KEYS.map((entry): Row => ({ id: `control:${entry.what}`, name: t(entry.what), keys: [entry.keys], where: t('shell.keyChoice.controls') })),
  ];
  const found = rows.filter((row) => shortcutMatches(query, row)).sort((one, other) => Number(other.where === here) - Number(one.where === here));
  const columns: Column<Row>[] = [
    { id: 'name', header: t('shell.keyChoice.actionColumn'), cell: (row, shown) => <div className="flex min-w-0 flex-col gap-0.5">
      <span>{row.name}</span>{!shown.has(WHERE_COLUMN) && row.where !== here && <span className="text-sm text-text-muted">{row.where}</span>}
    </div> },
    { id: 'keys', header: t('shell.keyChoice.keysColumn'), className: 'min-w-64', cell: (row) => row.action !== undefined ? <ActionRow id={row.action} />
      : <span className="flex items-center gap-1 text-text-subtle">{row.keys.map((keys, index) => <span key={keys} className="inline-flex items-center gap-1">{index > 0 && <span aria-hidden>/</span>}<Kbd shortcut={keys} /></span>)}</span> },
    { id: WHERE_COLUMN, header: t('shell.keyChoice.whereColumn'), from: 'md', cell: (row) => <span className="text-sm text-text-muted">{row.where}</span> },
  ];
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="sticky top-0 z-10 bg-surface-raised pb-2">
        <Input ref={search} type="search" data-autofocus label={t('shell.keyChoice.search')} hideLabel placeholder={t('shell.keyChoice.searchPlaceholder')} value={query} onChange={(event) => setQuery(event.target.value)} />
      </div>
      {found.length === 0 ? <EmptyState compact title={t('shell.keyChoice.nothing', { query })} action={<Button size="sm" onClick={() => { setQuery(''); search.current?.focus(); }}>{t('shell.keyChoice.clearSearch')}</Button>} />
        : <Table label={t('shell.keyChoice.table')} columns={columns} rows={found} rowKey={(row) => row.id} />}
    </div>
  );
}
