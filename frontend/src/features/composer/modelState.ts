/** What a thread runs with in speed, access and context, worked out from what its agent publishes, what its model's entry opens and what the thread itself chose; and what a change of model leaves of those choices.
 *
 * Two things are kept apart here and never read for each other. What a model may be run with is its entry at
 * the provider (`ModelParameters`: the speeds and programs a manager switched on, the largest context, and
 * what a thread starts with). What a thread does run with is its own choice among that, which is stored as
 * the thread's `agent.params`, or none, in which case it runs with what the model starts with. A parameter
 * is told apart by what its definition says (its options come from the model's speeds, from its access
 * programs, or its range ends at the model's largest context) and never by its key, which is the adapter's.
 */
import type { ModelParameters, ParameterView, Rate, ServiceTier, SettingLock, ThreadParameters } from '@/generated/api-types';

export type ChoiceKind = 'speed' | 'access';
export type Kind = ChoiceKind | 'context';

/** What a parameter's definition says, cut down to what the control reads; anything else it holds is left alone. */
export interface ParamDef {
  key: string;
  title: string;
  description: string;
  /** Which list of the model its options come from. */
  options: 'service_tiers' | 'access_programs' | 'efforts' | null;
  /** The range of a number, and whether its most is the model's largest context. */
  range: { least: number; mostFromModel: boolean; most: number } | null;
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** A definition as the adapter published it, read with care: one that has no key is none. */
export function readDef(def: unknown): ParamDef | null {
  if (!isRecord(def) || typeof def.key !== 'string') return null;
  const options = isRecord(def.options) && typeof def.options.from_model === 'string' ? def.options.from_model : null;
  const range = isRecord(def.range) && typeof def.range.min === 'number' ? def.range : null;
  const most = range === null ? null : range.max;
  return {
    key: def.key,
    title: typeof def.title === 'string' ? def.title : def.key,
    description: typeof def.description === 'string' ? def.description : '',
    options: options === 'service_tiers' || options === 'access_programs' || options === 'efforts' ? options : null,
    range:
      range === null || typeof range.min !== 'number'
        ? null
        : { least: range.min, mostFromModel: isRecord(most) && most.from_model === 'context_max', most: typeof most === 'number' ? most : 0 },
  };
}

/** Which of the three things a thread has one of a parameter is; none for any other. */
export function kindOf(def: ParamDef): Kind | null {
  if (def.options === 'service_tiers') return 'speed';
  if (def.options === 'access_programs') return 'access';
  if (def.range?.mostFromModel === true) return 'context';
  return null;
}

/** Where a thread's value comes from: its own choice, a place above it, or nothing, so that it runs with what its model starts with. */
export type From = 'chosen' | 'above' | 'start';

interface Common {
  key: string;
  title: string;
  /** The adapter's words on what the parameter does. */
  description: string;
  apply: ParameterView['apply'];
  /** Whether the reader may change it. */
  writable: boolean;
  lockedBy: SettingLock | null;
  /** The place a value that is set above the thread comes from, where the API names it. */
  place: string | null;
  from: From;
  /** A choice is stored that the model's entry no longer opens: it is not sent, and the thread runs with the start. */
  stale: boolean;
}

export interface SpeedOrAccess extends Common {
  kind: ChoiceKind;
  /** What the model's managers switched on, beside the ordinary speed or no program. */
  open: { id: string; label: string }[];
  /** What a thread starts with where nothing is chosen; none is the ordinary speed or no program. */
  start: string | null;
  /** The thread's own choice, where the entry still opens it. */
  chosen: string | null;
  /** What it runs with now. */
  force: string | null;
}

export interface ContextState extends Common {
  kind: 'context';
  /** The least a context may be, as the adapter's parameter declares it. */
  least: number;
  /** The model's own window. */
  window: number | null;
  /** The largest the model's managers opened. */
  largest: number;
  start: number;
  chosen: number | null;
  force: number;
  rate: Rate | null;
}

export type State = SpeedOrAccess | ContextState;

const isWhole = (value: unknown): value is number => typeof value === 'number' && Number.isInteger(value);

/** The speed a model starts with: the entry's own start, so long as it is still switched on; none is the ordinary speed. */
export function speedStart(model: ModelParameters): string | null {
  const named = model.default_service_tier;
  return named !== undefined && named !== null && (model.service_tiers ?? []).some((tier) => tier.id === named) ? named : null;
}

/** The access program a model starts with: the entry's own start, so long as it is still switched on; none is no program. */
export function programStart(model: ModelParameters): string | null {
  const named = model.default_access_program;
  return named !== undefined && named !== null && (model.access_programs ?? []).includes(named) ? named : null;
}

/** The context a model starts with: the entry's own start where it lies within what is open; otherwise all that is open. */
export function contextStart(model: ModelParameters): number | null {
  const largest = model.context_max;
  if (largest === undefined || largest === null || largest < 1) return null;
  const named = model.default_context;
  return named !== undefined && named !== null && named >= 1 && named <= largest ? named : largest;
}

/** Whether a number is a context the model opens and the adapter takes. */
export const contextFits = (value: unknown, model: ModelParameters, least: number): value is number => {
  const largest = model.context_max;
  return isWhole(value) && largest !== undefined && largest !== null && value >= least && value <= largest;
};

const tiersOf = (model: ModelParameters): ServiceTier[] => model.service_tiers ?? [];

/**
 * The speed, the access program and the context of a thread, each with what it runs with and where that comes
 * from. `own` is the thread's own choices when they are known (what they are the object `agent.params`); without
 * it a value that the parameter shows is taken to be the thread's own where its source says the thread.
 */
export function statesOf(parameters: ThreadParameters, own: Readonly<Record<string, unknown>> | null): State[] {
  const model = parameters.model.options.find((option) => option.model_name === parameters.model.value)?.parameters;
  if (model === undefined) return [];
  const states: State[] = [];
  for (const view of parameters.parameters) {
    const def = readDef(view.def);
    const kind = def === null ? null : kindOf(def);
    if (def === null || kind === null) continue;
    // A member the thread does not name itself runs with what a place above names, as the view says it.
    const ownNamed = own !== null && def.key in own;
    const stored: unknown = own !== null && ownNamed ? own[def.key] : view.value;
    const heldHere = own === null ? view.source.scope === 'thread' : ownNamed;
    // None of a speed or a program is a choice of its own, stored as null where the member is named.
    const namedNone = stored === null && (ownNamed || view.source.scope !== 'default');
    const common = {
      key: def.key,
      title: def.title,
      description: def.description,
      apply: view.apply,
      writable: view.writable,
      lockedBy: view.locked_by,
      place: view.source.node?.name ?? null,
    };
    const named = stored !== undefined && stored !== null;
    if (kind === 'context') {
      const largest = model.context_max;
      const start = contextStart(model);
      if (largest === undefined || largest === null || start === null || def.range === null) continue;
      const open = contextFits(stored, model, def.range.least);
      states.push({
        ...common,
        kind,
        least: def.range.least,
        window: model.context_window ?? null,
        largest,
        start,
        chosen: open ? stored : null,
        force: open ? stored : start,
        rate: model.rate ?? null,
        from: open ? (heldHere ? 'chosen' : 'above') : 'start',
        stale: named && !open,
      });
      continue;
    }
    const list = kind === 'speed' ? tiersOf(model).map((tier) => ({ id: tier.id, label: tier.label })) : (model.access_programs ?? []).map((program) => ({ id: program, label: program }));
    const start = kind === 'speed' ? speedStart(model) : programStart(model);
    const open = typeof stored === 'string' && list.some((one) => one.id === stored);
    states.push({
      ...common,
      kind,
      open: list,
      start,
      chosen: open ? stored : null,
      force: open ? stored : namedNone ? null : start,
      from: open || namedNone ? (heldHere ? 'chosen' : 'above') : 'start',
      stale: named && !open,
    });
  }
  return states;
}

/** The label of what a choice among speeds or programs runs with: the name of the speed or program, or the word for none. */
export function forceLabel(state: SpeedOrAccess): string | null {
  return state.force === null ? null : (state.open.find((one) => one.id === state.force)?.label ?? state.force);
}

/**
 * What is written as the thread's own choices once `state` is given `value`: the object without the member when the
 * value is what the model starts with; with the member as null where none of a speed or a program is chosen while the
 * model starts with one (the ordinary speed, no program); else with the value.
 */
export function withChoice(own: Readonly<Record<string, unknown>>, state: State, value: string | number | null): Record<string, unknown> {
  const { [state.key]: _gone, ...rest } = own;
  if (value === state.start) return rest;
  if (value === null) return state.kind === 'context' ? rest : { ...rest, [state.key]: null };
  return { ...rest, [state.key]: value };
}

/** What the thread's choices come to after the model is changed to `next`: a choice the new model does not open is dropped, one that still fits is kept. */
export interface Dropped {
  kind: Kind;
  key: string;
  /** What the thread had chosen: the speed's or program's name, or the size. */
  was: string | number;
  /** What it runs with instead: the new model's start. */
  becomes: string | number | null;
}

/** The choices a thread has made that a model does not open, and what each is replaced by. */
export function droppedFor(states: readonly State[], own: Readonly<Record<string, unknown>>, next: ModelParameters): Dropped[] {
  const dropped: Dropped[] = [];
  for (const state of states) {
    const choice = own[state.key];
    if (choice === undefined || choice === null) continue;
    const word = typeof choice === 'string' ? choice : typeof choice === 'number' ? String(choice) : '';
    if (state.kind === 'context') {
      if (!contextFits(choice, next, state.least)) dropped.push({ kind: 'context', key: state.key, was: isWhole(choice) ? choice : word, becomes: contextStart(next) });
    } else if (state.kind === 'speed') {
      if (!tiersOf(next).some((tier) => tier.id === choice)) dropped.push({ kind: 'speed', key: state.key, was: state.open.find((tier) => tier.id === choice)?.label ?? word, becomes: speedStart(next) });
    } else if (!(next.access_programs ?? []).includes(word)) {
      dropped.push({ kind: 'access', key: state.key, was: word, becomes: programStart(next) });
    }
  }
  return dropped;
}

/** The thread's choices without those a change of model drops. */
export function withoutDropped(own: Readonly<Record<string, unknown>>, dropped: readonly Dropped[]): Record<string, unknown> {
  const gone = new Set(dropped.map((one) => one.key));
  return Object.fromEntries(Object.entries(own).filter(([key]) => !gone.has(key)));
}

/** How many times a rate counts a speed: its multiple as a number, or none where the rate says nothing of it. */
export function speedMultiple(rate: Pick<Rate, 'tiers'> | null | undefined, speed: string): number | null {
  const given = rate?.tiers?.[speed];
  if (given === undefined) return null;
  const multiple = Number(given);
  return Number.isFinite(multiple) && multiple > 0 ? multiple : null;
}

/** The sentence of a person who has chosen: whom a choice was made by is known only from the lines of the thread's history. */
export interface Chooser {
  name: string;
}

/**
 * Who last changed a choice of the thread, from the lines of its history that say a setting changed. A line
 * names the setting it changed; the choices are members of one object, so the line is read for the member:
 * the line counts when the member differs between what it was and what it became.
 */
export function chooserOf(
  lines: readonly { key: string; from: unknown; to: unknown; by: { display_name: string } }[],
  member: string,
): Chooser | null {
  for (let at = lines.length - 1; at >= 0; at -= 1) {
    const line = lines[at];
    if (line === undefined) continue;
    // The server writes one line for each member that changed, keyed by the parameters and the member.
    if (line.key === member || line.key === `agent.params.${member}`) return { name: line.by.display_name };
    if (line.key === 'agent.params' && isRecord(line.to)) {
      const before = isRecord(line.from) ? line.from[member] : undefined;
      if (line.to[member] !== before) return { name: line.by.display_name };
    }
  }
  return null;
}
