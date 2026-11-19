/** What a person has chosen in the wizard of a fork or a switch, and the request that asks for exactly that.
 *
 * The wizard begins from nothing chosen: every member left out stays as the thread has it, and a request
 * carries only what the person changed. So the plan the server answers says what that changes, and a thread that
 * is asked to change nothing is told so ("nothing to change"), instead of being sent its own values back.
 * The request is built here and nowhere else.
 */
import type { ArrangementType, ChangeOptions, CopyInto, FileSelection, ForkEnvironment, ForkHistory, ForkRequest, MountChoice, MountFate, PlanMount, ThreadAccess } from '@/generated/api-types';

/** What the person chose for one mount; a mount without one takes its default. */
export interface MountPick {
  fate: MountFate;
  into: CopyInto | null;
}

/** The picks with one mount's changed: a pick of `null` gives the mount back to its default. */
export function withPick(picks: Readonly<Record<string, MountPick>>, mount: string, pick: MountPick | null): Record<string, MountPick> {
  const others = Object.fromEntries(Object.entries(picks).filter(([id]) => id !== mount));
  return pick === null ? others : { ...others, [mount]: pick };
}

/** A shared fate changes the editable rows together; null restores each mount's own default. */
export function withFate(picks: Readonly<Record<string, MountPick>>, mounts: readonly PlanMount[], fate: MountFate | null): Record<string, MountPick> {
  let next = { ...picks };
  for (const mount of mounts) {
    if (mount.fates.length <= 1) continue;
    next = withPick(next, mount.mount.id, fate === null || fate === mount.default_fate ? null : { fate, into: null });
  }
  return next;
}

/** The request's members for what the person chose: none for mounts left to their default. */
export function mountChoicesOf(picks: Readonly<Record<string, MountPick>>): MountChoice[] {
  return Object.entries(picks).map(([mount_id, pick]) => ({ mount_id, fate: pick.fate, ...(pick.into === null ? {} : { into: pick.into }) }));
}

/** Which files go along into another environment: all, none, or those that match what is written. */
export type FileChoice = { mode: 'all' } | { mode: 'none' } | { mode: 'selection'; include: string; exclude: string };

/** Where the result runs. */
export type WhereChoice =
  | { kind: 'same' }
  | { kind: 'use'; id: string; files: FileChoice }
  | { kind: 'new'; hostId: string | null; name: string; files: FileChoice; home: boolean; privateViews: boolean; onlyMe: boolean };

/** How much of the history a fork or a switch carries: all of it, up to one turn (what forking from a message asks), a range, a summary, none. */
export type HistoryChoice = 'all' | 'upto' | 'range' | 'summary' | 'none';

/** The page a fork starts with: the page of the thread it was taken from, or the defaults of where it is made, or its maker's alone. */
export type PageChoice = 'original' | 'place' | 'private';

/** The parameters of the change, each left alone until the person changes it. */
export interface OptionChoices {
  activeTurn: 'wait' | 'interrupt' | null;
  waitMinutes: number | null;
  verifySecs: number | null;
  keepPrevious: boolean | null;
  stopProcesses: boolean | null;
}

export interface Choices {
  /** A new thread, or this thread on another side. */
  result: 'new' | 'replace';
  /** What it runs on: `null` is what the thread has. */
  providerId: string | null;
  agentId: string | null;
  model: string | null;
  /** The effort: `undefined` is what the thread has, `null` is none. */
  effort: string | null | undefined;
  arrangement: ArrangementType | null;
  where: WhereChoice;
  /** The directory the result works in; empty is the source's. */
  directory: string;
  /** The history; a `null` mode is what the place carries (`thread.switch_history`), which the request leaves to the server. */
  history: { mode: HistoryChoice | null; fromTurn: number | null; toTurn: number | null };
  /** Whether the agent's own notes go along; `null` leaves it to the default. */
  memory: boolean | null;
  page: PageChoice;
  name: string;
  options: OptionChoices;
  /** What becomes of each mount the person changed, by mount; one left out takes its default. */
  mounts: Readonly<Record<string, MountPick>>;
}

export const NO_OPTIONS: OptionChoices = { activeTurn: null, waitMinutes: null, verifySecs: null, keepPrevious: null, stopProcesses: null };

export function initialChoices(result: 'new' | 'replace', toTurn: number | null = null): Choices {
  return {
    result,
    providerId: null,
    agentId: null,
    model: null,
    effort: undefined,
    arrangement: null,
    where: { kind: 'same' },
    directory: '',
    // From the first message of the conversation there is nothing before it to take: the fork starts fresh.
    history: { mode: toTurn === null ? null : toTurn <= 0 ? 'none' : 'upto', fromTurn: null, toTurn: toTurn !== null && toTurn > 0 ? toTurn : null },
    memory: null,
    page: 'original',
    name: '',
    options: NO_OPTIONS,
    mounts: {},
  };
}

export const ALL_FILES: FileChoice = { mode: 'all' };

/** The patterns of a selection, one to a line, with the empty lines left out. */
export function patterns(text: string): string[] {
  return text
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '');
}

/** What the files are, as the request says them. */
export function selectionOf(files: FileChoice): FileSelection {
  if (files.mode === 'selection') {
    const exclude = patterns(files.exclude);
    return { mode: 'selection', include: patterns(files.include), ...(exclude.length === 0 ? {} : { exclude }) };
  }
  return { mode: files.mode };
}

function environmentOf(where: WhereChoice): ForkEnvironment | null {
  switch (where.kind) {
    case 'same':
      return null;
    case 'use':
      return { use: where.id, ...(where.files.mode === 'all' ? {} : { files: selectionOf(where.files) }) };
    case 'new':
      return {
        new: {
          ...(where.hostId === null ? { host_id: null } : { host_id: where.hostId }),
          ...(where.name.trim() === '' ? {} : { name: where.name.trim() }),
          ...(where.files.mode === 'all' ? {} : { files: selectionOf(where.files) }),
          ...(where.home ? { home: true } : {}),
          ...(where.privateViews ? {} : { private_views: false }),
          ...(where.onlyMe ? { access: 'only_me' as const } : {}),
        },
      };
  }
}

/** The options the person changed, as the request says them. */
export function optionsOf(options: OptionChoices): ChangeOptions | null {
  const made: ChangeOptions = {
    ...(options.activeTurn === null ? {} : { active_turn: options.activeTurn }),
    ...(options.waitMinutes === null ? {} : { wait_minutes: options.waitMinutes }),
    ...(options.verifySecs === null ? {} : { verify_timeout_secs: options.verifySecs }),
    ...(options.keepPrevious === null ? {} : { keep_previous: options.keepPrevious }),
    ...(options.stopProcesses === null ? {} : { stop_processes: options.stopProcesses }),
  };
  return Object.keys(made).length === 0 ? null : made;
}

function accessOf(page: PageChoice): ThreadAccess | null {
  if (page === 'place') return 'defaults';
  if (page === 'private') return 'private';
  return null;
}

/** The members of `to` that were changed. A provider or an agent that changes takes its model and its effort with it. */
function toOf(choices: Choices): ForkRequest['to'] {
  const moved = choices.providerId !== null || choices.agentId !== null;
  const to = {
    ...(choices.providerId === null ? {} : { provider_id: choices.providerId }),
    ...(choices.agentId === null ? {} : { agent_id: choices.agentId }),
    ...(choices.model === null ? {} : { model: choices.model }),
    ...(choices.effort === undefined ? {} : { effort: choices.effort }),
    ...(choices.arrangement === null ? {} : { arrangement: choices.arrangement }),
  };
  return moved || choices.model !== null || choices.arrangement !== null ? to : null;
}

/** The history the person chose, as the request says it; a request without one carries what the place carries. */
function historyOf(choices: Choices): ForkHistory | null {
  const { mode, fromTurn, toTurn } = choices.history;
  switch (mode) {
    case null:
      return null;
    case 'all':
      return { mode: 'all' };
    case 'upto':
      return { mode: 'range', ...(toTurn === null ? {} : { to_turn: toTurn }) };
    case 'range':
      return { mode: 'range', ...(fromTurn === null ? {} : { from_turn: fromTurn }), ...(toTurn === null ? {} : { to_turn: toTurn }) };
    case 'summary':
    case 'none':
      return { mode };
  }
}

/** The request that asks for what was chosen and nothing else. */
export function requestOf(choices: Choices, topicId: string | null = null): ForkRequest {
  const to = toOf(choices);
  const where = environmentOf(choices.where);
  const options = optionsOf(choices.options);
  const history = historyOf(choices);
  const access = choices.result === 'new' ? accessOf(choices.page) : null;
  const mounts = mountChoicesOf(choices.mounts);
  return {
    result: choices.result,
    ...(to === null ? {} : { to }),
    ...(where === null ? {} : { environment: where }),
    ...(choices.directory.trim() === '' ? {} : { directory: choices.directory.trim() }),
    ...(history === null ? {} : { history }),
    ...(choices.memory === null ? {} : { memory: choices.memory }),
    ...(choices.result === 'new' && choices.name.trim() !== '' ? { name: choices.name.trim() } : {}),
    ...(choices.result === 'new' && topicId !== null ? { topic_id: topicId } : {}),
    ...(access === null ? {} : { access }),
    ...(options === null ? {} : { options }),
    ...(mounts.length === 0 ? {} : { mounts }),
  };
}

/** Whether the choices change anything of where or on what the result runs: a switch that does not is not asked for a plan. */
export function changesSomething(choices: Choices): boolean {
  return toOf(choices) !== null || choices.where.kind !== 'same' || choices.directory.trim() !== '';
}

/** How many of the parameters differ from where they start, for the mark beside Options. */
export function changedOptions(options: OptionChoices): number {
  return Object.values(options).filter((value) => value !== null).length;
}
