/** The page as a person reads it, and the changes it makes: who stands in which of the three lines, which rows are advanced or administration, how a grid is laid out, and each change as the request that makes it.
 *
 * Everything here is worked out from the rows of a page and the catalogue of permissions, and from
 * nothing else: no permission, area or level is named in this file except the few the page itself
 * sets apart (the figures of the usage area belong with administration, and administration is the
 * areas that are about running a place and not about working in it). A holder stands in the highest of
 * the three everyday choices it holds completely, whatever it holds besides; a role is everybody who
 * holds it or a higher one, so it also holds what the roles below it are named in.
 */
import type {
  AccessChoice,
  Entry,
  Holder,
  HolderInput,
  NodeOrSystem,
  PageChange,
  PermissionCatalogueGetReply,
  PermissionDef,
  PermissionLevel,
  PermissionPage,
  PermissionRow,
  Role,
  RowProfile,
} from '@/generated/api-types';


// ---------------------------------------------------------------- the catalogue, indexed

/** The roles from the one who may least to the one who may most. */
export const ROLE_ORDER: readonly Role[] = ['viewer', 'interactor', 'developer', 'admin'];

/** The three everyday choices from the least to the most. */
export const CHOICE_ORDER: readonly AccessChoice[] = ['full_view', 'full_interact', 'full_manage'];

/** The least ordinary access request that covers all the permissions the server says are missing. */
export function leastChoice(catalogue: Catalogue, kind: string, permissions: readonly string[]): AccessChoice {
  return CHOICE_ORDER.find((choice) => permissions.every((permission) => catalogue.choice(choice, kind).includes(permission))) ?? 'full_view';
}

/** The catalogue of permissions, with what pages ask of it kept ready. */
export interface Catalogue {
  reply: PermissionCatalogueGetReply;
  /** Every permission by its key. */
  defs: ReadonlyMap<string, PermissionDef>;
  /** The permissions that exist on a kind of thing, in the order of the catalogue. */
  of: (kind: string) => PermissionDef[];
  /** The permissions one of the three choices names on a kind of thing; none when the choice does not exist for that kind. */
  choice: (choice: AccessChoice, kind: string) => string[];
  /** What the catalogue calls a choice. */
  choiceLabel: (choice: AccessChoice) => string;
}

export function indexCatalogue(reply: PermissionCatalogueGetReply): Catalogue {
  const defs = new Map(reply.permissions.map((def) => [def.name, def]));
  return {
    reply,
    defs,
    of: (kind) => reply.permissions.filter((def) => def.on.includes(kind) || kind === 'chat' && def.area === 'chat'),
    choice: (choice, kind) => kind === 'chat'
      ? reply.permissions.filter((def) => def.area === 'chat' && def.level === (choice === 'full_view' ? 'view' : choice === 'full_manage' ? 'manage' : 'interact')).map((def) => def.name)
      : reply.choices.find((entry) => entry.key === choice)?.permissions[kind] ?? [],
    choiceLabel: (choice) => reply.choices.find((entry) => entry.key === choice)?.label ?? choice,
  };
}

/** How high a level is in its area's ladder; `null` for the levels that are no step of it. */
const RANK: Record<PermissionLevel, number | null> = {
  view: 0,
  interact: 1,
  edit: 1,
  use: 1,
  manage: 2,
  own: null,
  advanced: null,
};

export const rankOf = (def: PermissionDef): number | null => RANK[def.level];

/** The areas that are about running a place and not about working in it: their rows sit under Administration. */
const ADMINISTRATION_AREAS: ReadonlySet<string> = new Set(['settings', 'usage', 'audit', 'members', 'directs', 'access']);

/** The one advanced row that belongs with the figures of its area and not with the acts apart. */
const FIGURES_ROW = 'usage.summary';

export type Section = 'area' | 'advanced' | 'administration';

/** Where a permission is drawn on a page. */
export function sectionOf(def: PermissionDef): Section {
  if (def.name === FIGURES_ROW) return 'administration';
  if (def.level === 'advanced') return 'advanced';
  return ADMINISTRATION_AREAS.has(def.area) ? 'administration' : 'area';
}

/** The kinds of resource there are: what has Use for its middle level and a home in an org. */
const RESOURCE_KINDS: ReadonlySet<string> = new Set(['provider', 'agent', 'environment', 'host', 'machine', 'template', 'mcp_server', 'mount', 'skill']);

/** How a kind of thing words its middle level: a node is interacted with, a resource used, a document edited. */
export type Middle = 'interact' | 'use' | 'edit';

export function middleOf(kind: string): Middle {
  if (kind === 'document') return 'edit';
  return RESOURCE_KINDS.has(kind) ? 'use' : 'interact';
}

// ---------------------------------------------------------------- holders

/** What tells one holder from another on a page. */
export type HolderKey = string;

/** The identifier a holder or the input that names it carries, whichever it is. */
export function holderKey(holder: Holder | HolderInput): HolderKey {
  switch (holder.kind) {
    case 'role': {
      const at = (holder as { at?: string | { id: string } | null }).at;
      return `role:${holder.role}@${typeof at === 'string' ? at : (at?.id ?? '')}`;
    }
    case 'group':
      return `group:${'group' in holder ? holder.group.id : holder.id}`;
    case 'user':
      return `user:${'user' in holder ? holder.user.id : holder.id}`;
    case 'node':
      return `node:${'node' in holder ? holder.node.id : holder.id}`;
    case 'members':
      return `members:${'node' in holder ? holder.node.id : holder.id}`;
    case 'agents':
      return `agents:${'node' in holder ? holder.node.id : holder.id}`;
    case 'everyone':
      return 'everyone';
  }
}

/** The input that names a holder the page shows, for a change that names it elsewhere. */
export function inputOf(holder: Holder): HolderInput {
  switch (holder.kind) {
    case 'role':
      return { kind: 'role', role: holder.role, ...(holder.at === null ? {} : { at: holder.at.id }) };
    case 'group':
      return { kind: 'group', id: holder.group.id };
    case 'user':
      return { kind: 'user', id: holder.user.id };
    case 'node':
      return { kind: 'node', id: holder.node.id };
    case 'members':
      return { kind: 'members', id: holder.node.id };
    case 'agents':
      return { kind: 'agents', id: holder.node.id };
    case 'everyone':
      return { kind: 'everyone' };
  }
}

/** One permission a holder holds on a page, and how: named in the row, or by the row following a place's defaults. */
export interface Hold {
  permission: string;
  /** The entry that names the holder in this row; `null` when the row follows. */
  entry: Entry | null;
  /** The place whose defaults give it; `null` when an entry does. */
  from: NodeOrSystem | null;
}

/** A holder with everything it holds on the rows given. */
export interface PageHolder {
  key: HolderKey;
  holder: Holder;
  holds: Hold[];
}

/** Whose hold a row counts for. A row that a lock above decides holds what the lock says, and what was stored for it counts for nothing. */
export function holdsOfRow(row: PermissionRow): { holder: Holder; hold: Hold }[] {
  const found: { holder: Holder; hold: Hold }[] = [];
  const decided = row.locked !== null && !row.locked.here;
  if (!decided) {
    for (const entry of row.entries) found.push({ holder: entry.holder, hold: { permission: row.permission, entry, from: null } });
  }
  if ((row.inherit || decided) && row.inherited !== null) {
    for (const holder of row.inherited.holders) found.push({ holder, hold: { permission: row.permission, entry: null, from: row.inherited.from } });
  }
  return found;
}

/** Every holder of the rows, with what each holds. */
export function holdersOf(rows: readonly PermissionRow[]): Map<HolderKey, PageHolder> {
  const holders = new Map<HolderKey, PageHolder>();
  for (const row of rows) {
    for (const { holder, hold } of holdsOfRow(row)) {
      const key = holderKey(holder);
      const known = holders.get(key);
      if (known === undefined) holders.set(key, { key, holder, holds: [hold] });
      else known.holds.push(hold);
    }
  }
  return holders;
}

// ---------------------------------------------------------------- the three lines

export type LineId = 'manage' | 'interact' | 'view' | 'custom';

/** A holder as a line shows it. */
export interface LineHolder {
  key: HolderKey;
  holder: Holder;
  /** Everything it holds on the rows of the page that count for the lines. */
  holds: Hold[];
  /** The entries behind it, each once: what taking it out removes. */
  entries: Entry[];
  /** The places whose defaults give it some of what it holds. */
  from: NodeOrSystem[];
  /** The first end among its entries, when any has one. */
  until: string | null;
}

export type Lines = Record<LineId, LineHolder[]>;

/** The permissions of a set with everything lower in the ladder of their areas, so that holding Manage is also holding View. */
function withLower(held: ReadonlySet<string>, among: ReadonlyMap<string, PermissionDef>): Set<string> {
  const all = new Set(held);
  for (const name of held) {
    const def = among.get(name);
    const rank = def === undefined ? null : rankOf(def);
    if (def === undefined || rank === null) continue;
    for (const other of among.values()) {
      const otherRank = rankOf(other);
      if (other.area === def.area && otherRank !== null && otherRank < rank) all.add(other.name);
    }
  }
  return all;
}

const sameNodeOrSystem = (one: NodeOrSystem, other: NodeOrSystem): boolean =>
  typeof one === 'string' || typeof other === 'string' ? one === other : one.id === other.id;

function lineHolder(source: PageHolder, among: ReadonlySet<string> | null): LineHolder {
  const holds = among === null ? source.holds : source.holds.filter((hold) => among.has(hold.permission));
  const entries: Entry[] = [];
  const from: NodeOrSystem[] = [];
  for (const hold of holds) {
    if (hold.entry !== null && !entries.some((entry) => entry.id === hold.entry?.id)) entries.push(hold.entry);
    if (hold.from !== null && !from.some((place) => hold.from !== null && sameNodeOrSystem(place, hold.from))) from.push(hold.from);
  }
  const ends = entries.map((entry) => entry.expires_at).filter((end): end is string => end !== null).sort();
  return { key: source.key, holder: source.holder, holds, entries, from, until: ends[0] ?? null };
}

/**
 * Who stands in which line. `kind` is the kind of thing the page is of, as the catalogue names it.
 * A holder is in the highest of the three choices all of whose permissions it holds (a higher level
 * of an area is also every level below it, and a role is also the roles below it); one that holds
 * something of the lines but not a whole choice is Custom; one that holds nothing of them is in none.
 */
export function linesOf(page: PermissionPage, catalogue: Catalogue, kind: string): Lines {
  const lines: Lines = { manage: [], interact: [], view: [], custom: [] };
  const present = new Map<string, PermissionDef>();
  for (const row of page.rows) {
    const def = catalogue.defs.get(row.permission);
    if (def !== undefined) present.set(row.permission, def);
  }
  const needs = {
    full_manage: new Set(catalogue.choice('full_manage', kind).filter((name) => present.has(name))),
    full_interact: new Set(catalogue.choice('full_interact', kind).filter((name) => present.has(name))),
    full_view: new Set(catalogue.choice('full_view', kind).filter((name) => present.has(name))),
  } satisfies Record<AccessChoice, Set<string>>;
  // Every row of an area counts for the lines, so that somebody who holds only the Manage of one door is listed, as Custom, and not left out.
  const counted = new Set([...present.values()].filter((def) => sectionOf(def) === 'area').map((def) => def.name));
  const holders = holdersOf(page.rows);

  const ownOf = (holder: PageHolder): Set<string> => new Set(holder.holds.map((hold) => hold.permission).filter((name) => counted.has(name)));

  // A role is everybody who holds it or a higher one: what the roles below it are named in is also its own.
  const roleHolders = [...holders.values()].filter((holder) => holder.holder.kind === 'role');
  const effective = new Map<HolderKey, Set<string>>();
  const placeOf = (holder: PageHolder) => (holder.holder.kind === 'role' ? (holder.holder.at?.id ?? '') : '');
  for (const place of new Set(roleHolders.map(placeOf))) {
    let sofar = new Set<string>();
    for (const role of ROLE_ORDER) {
      const named = roleHolders.find((holder) => placeOf(holder) === place && holder.holder.kind === 'role' && holder.holder.role === role);
      if (named !== undefined) sofar = new Set([...sofar, ...ownOf(named)]);
      if (named !== undefined) effective.set(named.key, withLower(sofar, present));
    }
  }

  const placed: { holder: PageHolder; line: LineId }[] = [];
  for (const holder of holders.values()) {
    const own = ownOf(holder);
    if (own.size === 0) continue;
    const held = effective.get(holder.key) ?? withLower(own, present);
    const complete = (['full_manage', 'full_interact', 'full_view'] as const).find((choice) => needs[choice].size > 0 && [...needs[choice]].every((name) => held.has(name)));
    const line: LineId = complete === 'full_manage' ? 'manage' : complete === 'full_interact' ? 'interact' : complete === 'full_view' ? 'view' : 'custom';
    placed.push({ holder, line });
  }

  // Of the roles in one line only the lowest is drawn: it is everybody above it as well.
  const lowestInLine = new Map<string, number>();
  for (const { holder, line } of placed) {
    if (holder.holder.kind !== 'role') continue;
    const slot = `${placeOf(holder)}|${line}`;
    const order = ROLE_ORDER.indexOf(holder.holder.role);
    lowestInLine.set(slot, Math.min(lowestInLine.get(slot) ?? order, order));
  }
  for (const { holder, line } of placed) {
    if (holder.holder.kind === 'role' && line !== 'custom') {
      const lowest = lowestInLine.get(`${placeOf(holder)}|${line}`);
      if (lowest !== undefined && ROLE_ORDER.indexOf(holder.holder.role) > lowest) continue;
    }
    lines[line].push(lineHolder(holder, counted));
  }

  // The same holders in the same order whatever the order the rows came in: roles first from the highest, then groups, then people.
  const KIND_ORDER: Record<Holder['kind'], number> = { role: 0, everyone: 1, members: 2, node: 3, agents: 4, group: 5, user: 6 };
  for (const id of ['manage', 'interact', 'view', 'custom'] as const) {
    lines[id].sort((one, other) => {
      const kinds = KIND_ORDER[one.holder.kind] - KIND_ORDER[other.holder.kind];
      if (kinds !== 0) return kinds;
      if (one.holder.kind === 'role' && other.holder.kind === 'role') return ROLE_ORDER.indexOf(other.holder.role) - ROLE_ORDER.indexOf(one.holder.role);
      return 0;
    });
  }
  return lines;
}

/** The permissions a Custom holder holds among those the lines count, by the catalogue's names for them. */
export function customHeld(holder: LineHolder, nameOf: (permission: string) => string): string[] {
  return [...new Set(holder.holds.map((hold) => hold.permission))].map(nameOf);
}

// ---------------------------------------------------------------- where the page stands against its place

/** The place whose defaults most of the page follows; `null` when it follows none that names anybody. */
export function followedPlace(rows: readonly PermissionRow[]): NodeOrSystem | null {
  const counts = new Map<string, { place: NodeOrSystem; count: number }>();
  for (const row of rows) {
    const from = row.inherited?.from;
    if (!row.inherit || from === null || from === undefined) continue;
    const key = typeof from === 'string' ? from : from.id;
    const known = counts.get(key);
    if (known === undefined) counts.set(key, { place: from, count: 1 });
    else known.count += 1;
  }
  return [...counts.values()].sort((one, other) => other.count - one.count)[0]?.place ?? null;
}

/**
 * The place a page follows: the nearest place above the thing, whose defaults its rows start from. The
 * server names the places whose admins hold the page, nearest first, and the first of them is that place.
 */
export function containerOf(page: PermissionPage): NodeOrSystem | null {
  return page.admins.of[0] ?? null;
}

/** How many rows of the page are cut loose from the defaults. */
export const cutLooseCount = (rows: readonly PermissionRow[]): number => rows.filter((row) => !row.inherit).length;

// ---------------------------------------------------------------- the grid

/** One cell of the grid: a row of the page and what the catalogue says of its permission. */
export interface Cell {
  def: PermissionDef;
  row: PermissionRow;
}

/** An area of the grid: a cell for View, one for the middle level where the area has one, and one for Manage. */
export interface GridArea {
  area: string;
  view: Cell | null;
  middle: Cell | null;
  manage: Cell | null;
}

export interface Grid {
  areas: GridArea[];
  advanced: Cell[];
  administration: Cell[];
}

/**
 * Lays the rows of a page out as the grid of the full form, in the order of the catalogue.
 * `rowOf` finds the row of a permission (a defaults page has two for some, and the caller says which).
 */
export function gridOf(defs: readonly PermissionDef[], rowOf: (permission: string) => PermissionRow | undefined): Grid {
  const grid: Grid = { areas: [], advanced: [], administration: [] };
  for (const def of defs) {
    const row = rowOf(def.name);
    if (row === undefined || def.level === 'own') continue;
    const cell: Cell = { def, row };
    const section = sectionOf(def);
    if (section === 'advanced') grid.advanced.push(cell);
    else if (section === 'administration') grid.administration.push(cell);
    else {
      let area = grid.areas.find((entry) => entry.area === def.area);
      if (area === undefined) {
        area = { area: def.area, view: null, middle: null, manage: null };
        grid.areas.push(area);
      }
      const rank = rankOf(def);
      if (rank === 0) area.view = cell;
      else if (rank === 1) area.middle = cell;
      else if (rank === 2) area.manage = cell;
    }
  }
  return grid;
}

/** The row of a permission on a live page, which has one for each. */
export const rowOnPage = (page: PermissionPage) => (permission: string): PermissionRow | undefined =>
  page.rows.find((row) => row.permission === permission);

/** The row of a permission on a defaults page for one kind of environment: the row for that kind where the permission has two, else its only row. */
export const rowOnDefaults = (page: PermissionPage, profile: RowProfile) => (permission: string): PermissionRow | undefined => {
  const mine = page.rows.filter((row) => row.permission === permission);
  return mine.find((row) => row.profile === profile) ?? mine.find((row) => row.profile === 'any');
};

// ---------------------------------------------------------------- what a cell says

/** The holders that a row has now, each marked with whether an entry or the defaults gives it. */
export interface CellHolder {
  key: HolderKey;
  holder: Holder;
  entry: Entry | null;
  from: NodeOrSystem | null;
}

/** Whom a row names or follows, in the order they are drawn: those it names itself first, then those it inherits. */
export function cellHolders(row: PermissionRow): CellHolder[] {
  return holdsOfRow(row).map(({ holder, hold }) => ({ key: holderKey(holder), holder, entry: hold.entry, from: hold.from }));
}

// ---------------------------------------------------------------- changes

const ensure = <Value>(list: Value[] | undefined | null): Value[] => list ?? [];

/** Names people, groups and roles in every row one of the three choices covers. */
export function nameChange(
  page: PermissionPage,
  choice: AccessChoice,
  holders: readonly HolderInput[],
  more: { expires_at?: string | null; note?: string; export?: boolean } = {},
): PageChange {
  const change: PageChange = {
    expected_revision: page.revision,
    // Whoever is let read may take it away as a file unless the giver says otherwise; the server takes that as given, so it is told only when it is withheld.
    name: [
      {
        choice,
        holders: [...holders],
        ...(more.expires_at === undefined || more.expires_at === null ? {} : { expires_at: more.expires_at }),
        ...(more.note === undefined ? {} : { note: more.note }),
        ...(more.export === false ? { export: false } : {}),
      },
    ],
  };
  return change;
}

/** Takes entries out of the rows that hold them. */
export function removeChange(page: PermissionPage, entries: readonly { permission: string; entry: Entry; profile?: RowProfile }[]): PageChange {
  const rows: NonNullable<PageChange['rows']> = [];
  for (const { permission, entry, profile } of entries) {
    const wanted = profile === undefined || profile === 'any' ? null : profile;
    const found = rows.find((row) => row.permission === permission && (row.profile ?? null) === wanted);
    if (found === undefined) rows.push({ permission, ...(profile === undefined || profile === 'any' ? {} : { profile }), remove: [entry.id] });
    else found.remove = [...ensure(found.remove), entry.id];
  }
  return { expected_revision: page.revision, rows };
}

/** Freezes the holders of a followed row here, preserving its other direct entries. */
export function forkRowChange(row: PermissionRow, excluded: ReadonlySet<HolderKey> = new Set()): NonNullable<PageChange['rows']>[number] {
  const named = new Set(row.entries.map((entry) => holderKey(entry.holder)));
  const inherited = row.inherited?.entries ?? [];
  const described = new Set(inherited.map((entry) => holderKey(entry.holder)));
  const add: NonNullable<NonNullable<PageChange['rows']>[number]['add']> = row.inherit ? [
    ...inherited.filter((entry) => !excluded.has(holderKey(entry.holder)) && !named.has(holderKey(entry.holder))).map((entry) => ({ holder: inputOf(entry.holder), options: entry.options, ...(entry.expires_at === null ? {} : { expires_at: entry.expires_at }), ...(entry.note === '' ? {} : { note: entry.note }) })),
    ...(row.inherited?.holders ?? []).filter((holder) => !excluded.has(holderKey(holder)) && !named.has(holderKey(holder)) && !described.has(holderKey(holder))).map((holder) => ({ holder: inputOf(holder) })),
  ] : [];
  const remove = row.entries.filter((entry) => excluded.has(holderKey(entry.holder))).map((entry) => entry.id);
  return {
    permission: row.permission,
    ...(row.profile === 'any' ? {} : { profile: row.profile }),
    ...(row.inherit ? { inherit: false } : {}),
    ...(add.length === 0 ? {} : { add }),
    ...(remove.length === 0 ? {} : { remove }),
  };
}

/** Takes a named or followed holder out, cutting only the rows in which it stands. */
export function removeHolderChange(page: PermissionPage, holder: LineHolder): PageChange {
  const rows = page.rows.filter((row) => holdsOfRow(row).some((held) => holderKey(held.holder) === holder.key)).map((row) => forkRowChange(row, new Set([holder.key])));
  return { expected_revision: page.revision, rows };
}

/** Changes one holder's level where it stands. Other holders and rows outside its lines stay as they were. */
export function holderLevelChange(page: PermissionPage, catalogue: Catalogue, kind: string, holder: LineHolder, choice: AccessChoice): PageChange {
  const wanted = generalPermissions(catalogue, kind, choice === 'full_view' ? 'view' : choice === 'full_interact' ? 'interact' : 'manage');
  const lineRows = new Set(CHOICE_ORDER.flatMap((line) => catalogue.choice(line, kind)));
  const rows: NonNullable<PageChange['rows']> = [];
  for (const row of page.rows) {
    if (!lineRows.has(row.permission)) continue;
    const has = holdsOfRow(row).some((held) => holderKey(held.holder) === holder.key);
    const needs = wanted.has(row.permission);
    if (has === needs) continue;
    // A higher role still belongs to a lower role's audience. That untouched audience need not be copied or cut.
    if (needs && holder.holder.kind === 'role') {
      const targetRole = holder.holder;
      const covered = holdsOfRow(row).some(({ holder: other }) => other.kind === 'role' && other.at?.id === targetRole.at?.id && ROLE_ORDER.indexOf(other.role) < ROLE_ORDER.indexOf(targetRole.role));
      if (covered) continue;
    }
    const change = forkRowChange(row, needs ? new Set() : new Set([holder.key]));
    if (needs) change.add = [...ensure(change.add), { holder: inputOf(holder.holder) }];
    rows.push(change);
  }
  return { expected_revision: page.revision, rows };
}

/** Edits the constraints of a followed holder here, with the other holders frozen in the same request. */
export function editFollowedHolderChange(page: PermissionPage, row: PermissionRow, holder: Holder, more: { expires_at?: string | null; options?: unknown }): PageChange {
  const current = page.rows.find((candidate) => candidate.permission === row.permission && candidate.profile === row.profile) ?? row;
  const key = holderKey(holder);
  const change = forkRowChange(current);
  const local = current.entries.find((entry) => holderKey(entry.holder) === key);
  if (local !== undefined) change.update = [{ entry_id: local.id, ...more }];
  else {
    const copied = change.add?.find((entry) => holderKey(entry.holder) === key);
    change.add = [...ensure(change.add).filter((entry) => holderKey(entry.holder) !== key), { ...copied, holder: inputOf(holder), ...more }];
  }
  return { expected_revision: page.revision, rows: [change] };
}

/** Cuts a row loose from the defaults, or ties it back. */
export function followChange(page: PermissionPage, permission: string, profile: RowProfile, inherit: boolean): PageChange {
  return { expected_revision: page.revision, rows: [{ permission, ...(profile === 'any' ? {} : { profile }), inherit }] };
}

/** Names one holder in one row, with an end and the options the permission allows. */
export function addToRowChange(
  page: PermissionPage,
  permission: string,
  profile: RowProfile,
  holder: HolderInput,
  more: { expires_at?: string | null; options?: unknown } = {},
): PageChange {
  return {
    expected_revision: page.revision,
    rows: [
      {
        permission,
        ...(profile === 'any' ? {} : { profile }),
        add: [{ holder, ...(more.expires_at === undefined || more.expires_at === null ? {} : { expires_at: more.expires_at }), ...(more.options === undefined ? {} : { options: more.options }) }],
      },
    ],
  };
}

/** Changes the end or the options of an entry that exists. A `null` end takes the end away. */
export function updateEntryChange(
  page: PermissionPage,
  permission: string,
  profile: RowProfile,
  entryId: string,
  more: { expires_at?: string | null; options?: unknown },
): PageChange {
  return {
    expected_revision: page.revision,
    rows: [
      {
        permission,
        ...(profile === 'any' ? {} : { profile }),
        update: [{ entry_id: entryId, ...(more.expires_at === undefined ? {} : { expires_at: more.expires_at }), ...(more.options === undefined ? {} : { options: more.options }) }],
      },
    ],
  };
}

/** Sets or lifts the lock a defaults row puts on everything inside. */
export function lockChange(page: PermissionPage, permission: string, profile: RowProfile, locked: boolean): PageChange {
  return { expected_revision: page.revision, rows: [{ permission, ...(profile === 'any' ? {} : { profile }), locked }] };
}

/** Cuts every row loose, so that the page is held by its owners and by whom it names; or ties every row back to the defaults. */
export function wholeChange(page: PermissionPage, whole: 'private' | 'reset'): PageChange {
  return { expected_revision: page.revision, whole };
}

/** The audience of the page as a whole, apart from the people it names individually. */
export type GeneralAccessPreset = 'defaults' | 'view' | 'interact' | 'manage' | 'private';
export type GeneralAccess = GeneralAccessPreset | 'custom';

/** Everyone in a visible place is its explicit Viewer role, which includes the higher roles there. */
export const generalHolder = (placeId: string, kind = 'thread'): HolderInput => kind === 'org' ? { kind: 'members', id: placeId } : { kind: 'role', role: 'viewer', at: placeId };

/** Each full line up to the chosen level is named explicitly, including the lower lines. */
export function generalPermissions(catalogue: Catalogue, kind: string, preset: GeneralAccessPreset): Set<string> {
  const highest = preset === 'view' ? 0 : preset === 'interact' ? 1 : preset === 'manage' ? 2 : -1;
  return new Set(CHOICE_ORDER.slice(0, highest + 1).flatMap((choice) => catalogue.choice(choice, kind)));
}

/** The same cut and audience change at creation and on an existing page. Owners and other direct entries stay. */
export function generalAccessChange(
  catalogue: Catalogue,
  kind: string,
  placeId: string,
  preset: GeneralAccessPreset,
  page?: PermissionPage,
): PageChange {
  if (preset === 'defaults') return { ...(page === undefined ? {} : { expected_revision: page.revision }), whole: 'reset' };
  const audience = generalHolder(placeId, kind);
  const key = holderKey(audience);
  const named = generalPermissions(catalogue, kind, preset);
  const permissions = page === undefined ? catalogue.of(kind).map((def) => def.name) : page.rows.map((row) => row.permission);
  const rows: NonNullable<PageChange['rows']> = [...new Set(permissions)].filter((permission) => permission !== 'access.own').map((permission) => {
    const existing = page?.rows.filter((row) => row.permission === permission).flatMap((row) => row.entries) ?? [];
    const general = existing.filter((entry) => holderKey(entry.holder) === key);
    const remove = named.has(permission) ? [] : general.map((entry) => entry.id);
    return {
      permission,
      ...(page === undefined || page.rows.some((row) => row.permission === permission && row.inherit) ? { inherit: false } : {}),
      ...(remove.length === 0 ? {} : { remove }),
      ...(named.has(permission) && general.length === 0 ? { add: [{ holder: audience }] } : {}),
      ...(named.has(permission) && general.length > 0 ? { update: general.map((entry) => ({ entry_id: entry.id, expires_at: null, options: {} })) } : {}),
    };
  });
  // Row flags also work when the page is private already; whole:private refuses a page with nothing left to cut.
  return { ...(page === undefined ? {} : { expected_revision: page.revision }), rows };
}

const unrestrictedGeneralEntry = (entry: Entry): boolean => entry.expires_at === null && (
  entry.options === null || entry.options === undefined || typeof entry.options === 'object' && !Array.isArray(entry.options) && Object.keys(entry.options).length === 0
);

/** Which general audience the rows express. Individually named holders do not change that audience. */
export function generalAccessOf(page: PermissionPage, catalogue: Catalogue, kind: string, placeId: string): GeneralAccess {
  const rows = page.rows.filter((row) => row.permission !== 'access.own');
  if (rows.length === 0 || rows.some((row) => row.locked !== null && !row.locked.here)) return 'custom';
  const key = holderKey(generalHolder(placeId, kind));
  const general = rows.flatMap((row) => row.entries.filter((entry) => holderKey(entry.holder) === key).map((entry) => ({ row, entry })));
  if (rows.every((row) => row.inherit) && general.length === 0) return 'defaults';
  if (rows.some((row) => row.inherit)) return 'custom';
  if (general.length === 0) return 'private';
  if (general.some(({ entry }) => !unrestrictedGeneralEntry(entry))) return 'custom';
  const held = new Set(general.map(({ row }) => row.permission));
  for (const preset of ['view', 'interact', 'manage'] as const) {
    const expected = generalPermissions(catalogue, kind, preset);
    if (expected.size > 0 && held.size === expected.size && [...expected].every((permission) => held.has(permission))) return preset;
  }
  return 'custom';
}

/** Names owners, takes them out, or says who is first. */
export function ownersChange(page: PermissionPage, change: { add?: HolderInput[]; remove?: string[]; first?: string }): PageChange {
  return { expected_revision: page.revision, owners: change };
}

// ---------------------------------------------------------------- owners

/** The people among the owners: the Owners row also names groups, and the last person is the one who cannot be taken out. */
export function ownerPeople(page: PermissionPage): Entry[] {
  return page.owners.filter((entry) => entry.holder.kind === 'user');
}

/** Whether an owner's entry is the first owner's. */
export function isFirstOwner(page: PermissionPage, entry: Entry): boolean {
  return entry.holder.kind === 'user' && page.owner !== null && entry.holder.user.id === page.owner.id;
}

/** Whether an owner's entry may be taken out: not the only person there. */
export function mayTakeOutOwner(page: PermissionPage, entry: Entry): boolean {
  return !(entry.holder.kind === 'user' && ownerPeople(page).length <= 1);
}
