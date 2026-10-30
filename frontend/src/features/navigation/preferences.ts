/** What the left bar keeps for a person on every device: which of its sections are folded, which kinds of thing its tree shows, and whether topics that are done are hidden.
 *
 * It is one value among those the interface keeps for a person (`sidebar`), read once and changed as
 * a whole. A change shows at once and is put right if the server refuses it. The device keeps the
 * last value it saw, so that the bar is drawn as the person left it before the server has answered,
 * and nothing in it jumps when the answer comes.
 *
 * Where the tree begins and which of its rows are open are not among these: they belong to the work
 * under way on one device, and are kept there (`beginning.ts`).
 */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useMemo } from 'react';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { meStateKey, meStateQuery, setMeState } from '@/api/personal';
import { queryClient } from '@/api/query';
import { i18n } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { errorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';

/** The sections of the bar that fold. */
export type SectionId = 'starred' | 'recent' | 'projects';

const SECTIONS: readonly SectionId[] = ['starred', 'recent', 'projects'];

/** The kinds of thing the tree can show beside its places, each switched on or off by the reader. */
export type Layer = 'threads' | 'documents' | 'chats';

export const LAYERS: readonly Layer[] = ['threads', 'documents', 'chats'];

export interface SidebarChoices {
  participating?: boolean;
  /** The sections that show their heading alone. */
  folded: readonly SectionId[];
  /** Which kinds of thing stand in the tree beside its places. */
  show: Readonly<Record<Layer, boolean>>;
  /** Whether topics that are done are left out of the tree. */
  hideDone: boolean;
}

/**
 * How the bar stands for somebody who chose nothing: Recent is folded, since the finder shows the same
 * list with an empty field; the tree shows places, threads and documents, and no chats; a done topic
 * stays in view.
 */
export const SHIPPED_CHOICES: SidebarChoices = {
  folded: ['recent'],
  show: { threads: true, documents: true, chats: false },
  hideDone: false,
};

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** Reads the choices out of what is kept, whatever shape an older build or another client left there. */
export function readChoices(kept: unknown): SidebarChoices {
  if (!isRecord(kept)) return SHIPPED_CHOICES;
  const shown = isRecord(kept.show) ? kept.show : {};
  const show = { ...SHIPPED_CHOICES.show };
  for (const layer of LAYERS) {
    const value = shown[layer];
    if (typeof value === 'boolean') show[layer] = value;
  }
  return {
    ...(kept.participating === true ? { participating: true } : {}),
    folded: Array.isArray(kept.folded) ? SECTIONS.filter((section) => (kept.folded as unknown[]).includes(section)) : SHIPPED_CHOICES.folded,
    show,
    hideDone: typeof kept.hide_done === 'boolean' ? kept.hide_done : SHIPPED_CHOICES.hideDone,
  };
}

/** The value to keep after a change: the choices as they then stand, with whatever else is kept beside them left as it is. */
export function withChoices(kept: unknown, change: Partial<SidebarChoices>): Record<string, unknown> {
  const next = { ...readChoices(kept), ...change };
  return { ...(isRecord(kept) ? kept : {}), folded: [...next.folded], show: { ...next.show }, hide_done: next.hideDone, ...(next.participating === undefined ? {} : { participating: next.participating }) };
}

const storageKey = (userId: string) => `orch.${userId}.sidebar`;

function readDevice(userId: string | null): unknown {
  if (userId === null) return undefined;
  try {
    return JSON.parse(localStorage.getItem(storageKey(userId)) ?? 'null') as unknown;
  } catch {
    return undefined;
  }
}

function writeDevice(userId: string | null, kept: unknown): void {
  if (userId === null) return;
  try {
    localStorage.setItem(storageKey(userId), JSON.stringify(kept ?? null));
  } catch {
    // A device that keeps nothing draws the bar as shipped until the server has answered.
  }
}

/** What is kept at the moment, as far as this page knows: what the server said, and before it has, what the device saw last. */
function keptNow(): unknown {
  const state = queryClient.getQueryData<Record<string, unknown>>(meStateKey);
  return state === undefined ? readDevice(client.session.getState().userId) : state.sidebar;
}

/** The choices as they stand now, for an act that is no drawing: a key, an entry of a menu. */
export function sidebarChoices(): SidebarChoices {
  return readChoices(keptNow());
}

/** How many changes of this page are on their way to the server. */
let saving = 0;

/** Whether a change of this page is on its way to the server: word of a change that comes back over the person's feed meanwhile is this page's own. */
export function isSavingSidebar(): boolean {
  return saving > 0;
}

/**
 * Changes what the bar keeps: shown at once, kept on the device, sent to the server, and put back, with
 * a word why, if the server refuses it. `change` is given the choices as they stand and answers with
 * the ones that change.
 */
export function changeSidebar(change: (choices: SidebarChoices) => Partial<SidebarChoices>): void {
  const userId = client.session.getState().userId;
  const before = queryClient.getQueryData<Record<string, unknown>>(meStateKey);
  const next = withChoices(keptNow(), change(sidebarChoices()));
  // A reading that is under way would bring back the value before this change: it is let go.
  void queryClient.cancelQueries({ queryKey: meStateKey });
  queryClient.setQueryData<Record<string, unknown>>(meStateKey, { ...before, sidebar: next });
  writeDevice(userId, next);
  saving += 1;
  setMeState('sidebar', next)
    .catch((error: unknown) => {
      // What the server holds stands: the choice is put back, unless a later one has replaced it meanwhile.
      if (queryClient.getQueryData<Record<string, unknown>>(meStateKey)?.sidebar === next) {
        queryClient.setQueryData<Record<string, unknown>>(meStateKey, (state) => ({ ...state, sidebar: before?.sidebar }));
        writeDevice(userId, before?.sidebar);
      }
      const { locale, timeZone } = useAppearance.getState();
      toast({ tone: 'danger', title: i18n.t('navigation.notKept'), description: errorText(error, i18n.t.bind(i18n), { locale, timeZone }).message });
    })
    .finally(() => {
      saving -= 1;
    });
}

export interface SidebarPreferences extends SidebarChoices {
  isFolded: (section: SectionId) => boolean;
  setFolded: (section: SectionId, folded: boolean) => void;
  setShown: (layer: Layer, shown: boolean) => void;
  setHideDone: (hidden: boolean) => void;
  setParticipating: (on: boolean) => void;
}

const setFolded = (section: SectionId, on: boolean): void =>
  changeSidebar((choices) => {
    const others = choices.folded.filter((one) => one !== section);
    return { folded: on ? [...others, section] : others };
  });
const setShown = (layer: Layer, shown: boolean): void => changeSidebar((choices) => ({ show: { ...choices.show, [layer]: shown } }));
const setHideDone = (hidden: boolean): void => changeSidebar(() => ({ hideDone: hidden }));
const setParticipating = (on: boolean): void => changeSidebar(() => ({ participating: on }));

export function useSidebarPreferences(): SidebarPreferences {
  const userId = useStore(client.session, (state) => state.userId);
  const state = useQuery(meStateQuery);
  // What the server says once it has answered; until then what this device saw last.
  const kept = state.data === undefined ? readDevice(userId) : state.data.sidebar;
  const choices = useMemo(() => readChoices(kept), [kept]);

  // What the server holds is what the device draws from next time.
  const answered = state.data !== undefined;
  const served = state.data?.sidebar;
  useEffect(() => {
    if (answered) writeDevice(userId, served);
  }, [answered, served, userId]);

  return useMemo(() => ({ ...choices, isFolded: (section) => choices.folded.includes(section), setFolded, setShown, setHideDone, setParticipating }), [choices]);
}
