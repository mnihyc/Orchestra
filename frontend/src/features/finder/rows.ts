/** The rows the finder lists and the groups they stand in: what a row says, what Enter does with it, and how the groups are put in order.
 *
 * Everything here is plain data and functions of it, so that the order of the groups, what each shows
 * and what it says under its name can be tested without a page. The sources (names from the server,
 * commands, settings, help) make rows; the assembly puts the groups that answer in the order of how well
 * they answer, and cuts each to what a glance can hold.
 */
import type { TFunction } from 'i18next';
import type { LucideIcon } from 'lucide-react';

import type { OpenHow } from '@/features/finder/commands';
import { describe, type Destination, destinationOf, iconOf, placeKindOf, usernameOf, wayOf } from '@/features/finder/destinations';
import type { Mode } from '@/features/finder/modes';
import { markedRanges } from '@/features/finder/ranking';
import type { AdminAccess, NameHit, ThreadDisplay } from '@/generated/api-types';
import { adminAccessOf } from '@/ui/AsAdminMark';
import { labelOfName } from '@/features/search/labels';
import type { KnownPlaces } from '@/lib/placeLabel';
import type { NodeIconKind } from '@/ui/NodeIcon';
import type { FileSource } from '@/features/files/source';

export type GroupId = 'starred' | 'recent' | 'actions' | 'go' | 'people' | 'resources' | 'commands' | 'settings' | 'rows' | 'help' | 'conversations';

export interface FinderRow {
  /** An attachment keeps the same chip, context menu and viewer as its source message. */
  file?: FileSource;
  /** The personal-state identity of a real name row, distinct from its group/command id. */
  target?: { kind: NameHit['kind']; id: string };
  /** Unique in the list: it is also the value the list chooses by. */
  id: string;
  group: GroupId;
  title: string;
  adminAccess?: AdminAccess | null;
  /** The parts of the title that the letters typed fall on. */
  marks: readonly (readonly [number, number])[];
  /** What stands under the name: what it is, and where it lies or what it is set to. */
  detail: string;
  /** Localized current state, where the named kind records one. */
  state?: string;
  /** The server's activity clock, drawn with the shared RelativeTime. */
  lastActivity?: string;
  icon: LucideIcon;
  /** The place the row stands for, which is drawn with the icon of its kind in the colour it was given; absent for what is no place. */
  place?: { kind: NodeIconKind; id: string; color?: number | null; own?: string | null; display?: ThreadDisplay };
  archived?: boolean;
  starred?: boolean;
  /** Why it cannot be chosen; the row is then listed switched off, with these words. */
  disabled?: string;
  /** A word at the end of the row: what kind of help it is. */
  note?: string;
  /** The key that does the same. */
  shortcut?: string;
  /** Where it leads, for the address of a new browser tab; absent for a row that does something else. */
  destination?: Destination;
  /** Whether it can be opened beside. */
  beside?: boolean;
  /** What its second act (Alt+Enter) does where it is not opening beside: a place's opens its chat. */
  besideAct?: 'chat';
  /** Whether choosing it leaves the finder open: it changes what the finder shows, and goes nowhere. */
  keeps?: boolean;
  /** Does it. `how` says where the reader asked for it to open. */
  run: (how: OpenHow) => void;
}

/** A row and how well it answers: groups are put in order by their best row. */
export interface Scored {
  row: FinderRow;
  score: number;
}

export interface Group {
  id: GroupId;
  heading: string;
  rows: FinderRow[];
  /** How many more the group has than it shows, which a row at its end offers to show. */
  more: number;
}

/** What the rows are made with. */
export interface Env {
  t: TFunction;
  /** Goes to a destination: here, in a new tab of the browser, or beside. */
  go: (destination: Destination, how: OpenHow) => void;
  /** The short name of the org in view, for what is in the messages of an org. */
  orgSlug: string | null;
  projectId?: string | null;
  projectPath?: readonly string[];
  knownPlaces?: KnownPlaces;
  placeLabels?: ReadonlyMap<string, string>;
  /** Whether the router has a route. */
  exists: (to: string) => boolean;
  /** Shows the card of a person or a group, where there is no page of theirs to go to. */
  showCard: (hit: NameHit) => void;
}

/**
 * Whether a found chat is a row of its own. A place's chat in which nothing was said would only list the place twice
 * ("Fix login redirect · topic" and "· chat"): it is one only once something was said in it, and typing the word for a
 * chat makes no row of an empty one; the place's own row opens its chat with its second act (Alt+Enter). A direct or
 * group chat, and a place no tree held knows, are always rows.
 */
export function chatWorthARow(hit: NameHit, said: (nodeId: string) => boolean | null): boolean {
  if (hit.kind !== 'chat') return true;
  const place = hit.path.at(-1);
  if (place === undefined) return true;
  return said(place.id) !== false;
}

/** The row of a name: what it is, where it lies, and where it leads. */
export function rowOfName(hit: NameHit, group: GroupId, query: string, env: Env, starred = hit.starred ?? false): FinderRow {
  const described = describe(hit);
  const placeKind = placeKindOf(described);
  const destination = destinationOf(hit);
  const leads = destination !== null;
  const labels = env.placeLabels ?? new Map();
  const title = labelOfName(hit, labels);
  const username = hit.kind === 'user' ? usernameOf(hit) : null;
  const where = hit.kind === 'user' ? username === null ? '' : `@${username}` : wayOf(hit, env.t, labels);
  // A place's second act opens its chat: its screen with the chat panel open, empty or not.
  const chat = leads && (hit.kind === 'org' || hit.kind === 'project' || hit.kind === 'topic' || hit.kind === 'thread') ? { ...destination, search: { ...destination.search, panel: 'chat' } } : null;
  return {
    id: `${group}:${hit.kind}:${hit.id}`,
    target: { kind: hit.kind, id: hit.id },
    group,
    title,
    adminAccess: adminAccessOf(hit),
    marks: markedRanges(title, query),
    detail: [env.t(`finder.kinds.${described}`), where].filter((part) => part !== '').join(': '),
    ...(hit.state === undefined || hit.state === null ? {} : { state: env.t(`everything.states.${hit.state}`, { defaultValue: env.t('everything.unknown') }) }),
    ...(hit.last_activity === undefined || hit.last_activity === null ? {} : { lastActivity: hit.last_activity }),
    icon: iconOf(described),
    ...(placeKind === null ? {} : { place: { kind: placeKind, id: hit.id, ...(hit.thread_display === null || hit.thread_display === undefined ? {} : { display: hit.thread_display }), ...(group === 'starred' ? {} : { color: hit.color, own: hit.custom_color }) } }),
    archived: hit.archived,
    starred,
    ...(leads ? { destination } : {}),
    ...(leads ? {} : { keeps: true }),
    ...(chat === null ? (leads && (hit.kind === 'document' || hit.kind === 'chat') ? { beside: true } : {}) : { beside: true, besideAct: 'chat' as const }),
    run: (how) => {
      if (leads) env.go(how === 'beside' && chat !== null ? chat : destination, how);
      else env.showCard(hit);
    },
  };
}

// ---------------------------------------------------------------- putting the groups in order

/** Which groups a mode lists, and in which order they stand when they answer equally well. */
const GROUPS_OF: Record<Mode, readonly GroupId[]> = {
  all: ['starred', 'recent', 'actions', 'go', 'people', 'resources', 'commands', 'settings', 'rows', 'help', 'conversations'],
  commands: ['commands'],
  people: ['people'],
  settings: ['settings', 'rows'],
  help: ['help'],
  conversations: ['conversations'],
};

/** How many rows a group shows when others are shown beside it, and how many when it is alone. */
const LIMITS: Record<GroupId, { shared: number; alone: number }> = {
  starred: { shared: 5, alone: 5 },
  recent: { shared: 8, alone: 8 },
  actions: { shared: 2, alone: 2 },
  go: { shared: 8, alone: 20 },
  people: { shared: 4, alone: 20 },
  resources: { shared: 4, alone: 20 },
  commands: { shared: 5, alone: 30 },
  settings: { shared: 4, alone: 20 },
  rows: { shared: 3, alone: 20 },
  help: { shared: 4, alone: 20 },
  conversations: { shared: 1, alone: 8 },
};

/** What a group is called in the list. */
export const headingOf = (id: GroupId, t: TFunction): string => t(`finder.groups.${id}`);

export interface Assembly {
  mode: Mode;
  /** Nothing typed after the sign, if any. */
  empty: boolean;
  /** What each source answered, best first; a source that has not answered is absent. */
  answers: Partial<Record<GroupId, readonly Scored[]>>;
  /** The groups whose reader asked to see every row. */
  expanded: ReadonlySet<GroupId>;
  /** A full remote group's final row opens the complete paged inventory. */
  continuations?: Partial<Record<GroupId, FinderRow>>;
  t: TFunction;
}

/**
 * The groups that answer, in the order of how well they answer: the group whose best row scores highest
 * comes first, and groups that score alike keep the order of `GROUPS_OF`. With nothing typed there is no
 * score to go by, and the order is that list. A group that has no row is not listed at all.
 */
export function assemble({ mode, answers, expanded, empty, t, continuations = {} }: Assembly): Group[] {
  const listed = GROUPS_OF[mode].filter((id) => (answers[id]?.length ?? 0) > 0);
  const alone = listed.length === 1 || mode !== 'all';
  const groups = listed.map((id, order) => {
    const found = answers[id] ?? [];
    const limit = expanded.has(id) || mode === 'commands' ? found.length : alone ? LIMITS[id].alone : LIMITS[id].shared;
    const continuation = continuations[id];
    const rows = found.slice(0, limit).map(({ row }) => row);
    if (continuation !== undefined) rows.push(continuation);
    return {
      order,
      best: found[0]?.score ?? 0,
      group: { id, heading: headingOf(id, t), rows, more: continuation === undefined ? Math.max(0, found.length - limit) : 0 },
    };
  });
  if (!empty) groups.sort((one, other) => other.best - one.best || one.order - other.order);
  return groups.map(({ group }) => group);
}
