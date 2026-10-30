/** What a model's entry at its provider says it can be run with, as a form: efforts and the one a thread starts with, images, the speeds and access programs switched on and the one each starts with, the context, groups and rate; with the rules the server keeps to it, read at the field before anything is sent.
 *
 * The entry has three parts for what costs more or reaches further: what the model offers there (written
 * by discovery and opening nothing), what its managers opened (the lists and the largest context, which
 * only a manager writes) and what a thread starts with (the `default_*` members). A speed, an access
 * program or a larger context is open to threads only once a manager switched it on here. The form holds
 * what the manager typed as text, so that a half-written number is never lost; `parametersOf` turns it into
 * what the server is sent, over what the entry had, so that what discovery offered is kept as it was.
 */
import type { TFunction } from 'i18next';

import { ApiError } from '@/api/client';
import { isGroupName, GROUP_NAME_LONGEST, GROUPS_MOST } from '@/features/models/patterns';
import { readTokens } from '@/features/models/context';
import { formatNumber } from '@/lib/format';
import type { ModelParameters, Rate, ServiceTier } from '@/generated/api-types';

/** Where a thread on a model starts, in a context: with all that is open, with the model's own window, or with a number. */
export type ContextStart = 'all' | 'window' | 'number';

/** One step of a rate: a prompt above a size pays these prices instead. */
export interface StepForm {
  over: string;
  input: string;
  cached: string;
  output: string;
  write: string;
}

export interface EntryForm {
  efforts: string;
  defaultEffort: string;
  images: boolean;
  /** The model's own window. */
  window: string;
  /** The largest context a thread may be given; empty until a manager fills it. */
  largest: string;
  start: ContextStart;
  startNumber: string;
  /** The speeds that are switched on. */
  tiers: ServiceTier[];
  /** The one a thread starts with; empty is the ordinary speed. */
  defaultTier: string;
  /** The access programs that are switched on. */
  programs: string[];
  /** The one a thread starts with; empty is none. */
  defaultProgram: string;
  rate: { input: string; cached: string; output: string; write: string };
  /** The multiple of the rate that each speed is counted at, by the speed's id. */
  multipliers: Record<string, string>;
  steps: StepForm[];
  groups: string[];
}

/** What a field of the form can be refused for, by name. */
export type FieldName = 'routing' | 'display' | 'efforts' | 'defaultEffort' | 'window' | 'largest' | 'start' | 'tier' | 'program' | 'rate' | 'steps' | 'groups';
export type Faults = Partial<Record<FieldName, string>>;

const splitList = (text: string): string[] =>
  text
    .split(/[,\n]/)
    .map((part) => part.trim())
    .filter((part) => part !== '');

export const effortsOf = (form: Pick<EntryForm, 'efforts'>): string[] => splitList(form.efforts);

/** Which of the three starts an entry's context has. */
function startOf(parameters: ModelParameters): { start: ContextStart; number: string } {
  const named = parameters.default_context;
  if (named === null || named === undefined) return { start: 'all', number: '' };
  if (parameters.context_window !== null && parameters.context_window !== undefined && named === parameters.context_window) return { start: 'window', number: '' };
  return { start: 'number', number: String(named) };
}

const text = (value: number | null | undefined): string => (value === null || value === undefined ? '' : String(value));

/** The form of an entry as it is now. */
export function formOf(parameters: ModelParameters, groups: readonly string[] = []): EntryForm {
  const rate = parameters.rate;
  const start = startOf(parameters);
  return {
    efforts: (parameters.efforts ?? []).join(', '),
    defaultEffort: parameters.default_effort ?? '',
    images: parameters.input_images === true,
    window: text(parameters.context_window),
    largest: text(parameters.context_max),
    start: start.start,
    startNumber: start.number,
    tiers: parameters.service_tiers ?? [],
    defaultTier: parameters.default_service_tier ?? '',
    programs: parameters.access_programs ?? [],
    defaultProgram: parameters.default_access_program ?? '',
    rate: {
      input: rate?.input_per_mtok ?? '',
      cached: rate?.cached_input_per_mtok ?? '',
      output: rate?.output_per_mtok ?? '',
      write: rate?.cache_write_per_mtok ?? '',
    },
    multipliers: { ...(rate?.tiers ?? {}) },
    steps: (rate?.above ?? []).map((step) => ({
      over: String(step.context_tokens),
      input: step.input_per_mtok,
      cached: step.cached_input_per_mtok ?? '',
      output: step.output_per_mtok,
      write: step.cache_write_per_mtok ?? '',
    })),
    groups: [...groups],
  };
}

const DECIMAL = /^\d+(\.\d+)?$/;

/** Whether a number of tokens is written whole and at least one. */
const isSize = (value: string): boolean => {
  const tokens = readTokens(value);
  return tokens !== null && tokens >= 1;
};

/** What is wrong with a name given to a group, in words; null when nothing is. */
export function groupFault(name: string, t: TFunction): string | null {
  return isGroupName(name) ? null : t('models.fault.groupName', { longest: GROUP_NAME_LONGEST });
}

/** What is wrong with the form, in words, field by field: what the server would refuse, said before it is asked. */
export function faultsOf(form: EntryForm, t: TFunction, locale = 'en-US'): Faults {
  const found: Faults = {};
  const window = readTokens(form.window);
  const largest = readTokens(form.largest);
  if (form.window.trim() !== '' && !isSize(form.window)) found.window = t('models.fault.size');
  if (form.largest.trim() !== '' && !isSize(form.largest)) found.largest = t('models.fault.size');
  else if (window !== null && largest !== null && largest < window) found.largest = t('models.fault.largestBelowWindow', { window: formatNumber(window, locale) });
  if (largest !== null && form.start === 'number') {
    const started = readTokens(form.startNumber);
    if (!isSize(form.startNumber)) found.start = t('models.fault.size');
    else if (started !== null && started > largest) found.start = t('models.fault.startAboveLargest', { largest: formatNumber(largest, locale) });
  }
  if (largest !== null && form.start === 'window' && window === null) found.start = t('models.fault.startNeedsWindow');

  const prices = [form.rate.input, form.rate.cached, form.rate.output].map((price) => price.trim());
  const given = [
    ...prices,
    form.rate.write.trim(),
    ...Object.values(form.multipliers).map((value) => value.trim()),
    ...form.steps.flatMap((step) => [step.input, step.cached, step.output, step.write].map((price) => price.trim())),
  ].filter((price) => price !== '');
  if (given.some((price) => !DECIMAL.test(price))) found.rate = t('models.fault.rateInvalid');
  else if ((given.length > 0 || form.steps.length > 0) && prices.some((price) => price === '')) found.rate = t('models.fault.rateMissing');
  if (form.steps.some((step) => !isSize(step.over) || step.input.trim() === '' || step.output.trim() === '')) found.steps = t('models.fault.stepIncomplete');

  const bad = form.groups.find((name) => !isGroupName(name));
  if (bad !== undefined) found.groups = groupFault(bad, t) ?? undefined;
  else if (form.groups.length > GROUPS_MOST) found.groups = t('models.fault.groupsMost', { most: GROUPS_MOST });
  return found;
}

/** Whether a larger context is open in the form: its largest is filled. */
export const hasLargest = (form: Pick<EntryForm, 'largest'>): boolean => readTokens(form.largest) !== null;

/** The rate the form stands for, over the one the entry had; none when the three prices are not all given. */
function rateOf(form: EntryForm): Rate | undefined {
  const given = [form.rate.input, form.rate.cached, form.rate.output].every((price) => price.trim() !== '');
  if (!given) return undefined;
  const tiers = Object.fromEntries(
    form.tiers
      .map((tier) => [tier.id, (form.multipliers[tier.id] ?? '').trim()] as const)
      .filter(([, value]) => value !== ''),
  );
  const above = form.steps.map((step) => ({
    context_tokens: readTokens(step.over) ?? 0,
    input_per_mtok: step.input.trim(),
    output_per_mtok: step.output.trim(),
    ...(step.cached.trim() === '' ? {} : { cached_input_per_mtok: step.cached.trim() }),
    ...(step.write.trim() === '' ? {} : { cache_write_per_mtok: step.write.trim() }),
  }));
  return {
    input_per_mtok: form.rate.input.trim(),
    cached_input_per_mtok: form.rate.cached.trim(),
    output_per_mtok: form.rate.output.trim(),
    ...(form.rate.write.trim() === '' ? {} : { cache_write_per_mtok: form.rate.write.trim() }),
    ...(Object.keys(tiers).length === 0 ? {} : { tiers }),
    ...(above.length === 0 ? {} : { above }),
  };
}

/** The members of an entry that the form owns; whatever else the entry holds (what discovery offered) is kept as it was. */
const OWNED = [
  'efforts',
  'default_effort',
  'input_images',
  'service_tiers',
  'default_service_tier',
  'access_programs',
  'default_access_program',
  'context_window',
  'context_max',
  'default_context',
  'rate',
] as const satisfies readonly (keyof ModelParameters)[];

/** What the entry says once the form is saved: what is left empty is left out, so that an entry says only what a manager said. */
export function parametersOf(form: EntryForm, base: ModelParameters): ModelParameters {
  const owned: readonly string[] = OWNED;
  const kept = Object.fromEntries(Object.entries(base).filter(([member]) => !owned.includes(member))) as ModelParameters;
  const efforts = effortsOf(form);
  const window = readTokens(form.window);
  const largest = readTokens(form.largest);
  const started = form.start === 'window' ? window : form.start === 'number' ? readTokens(form.startNumber) : null;
  const rate = rateOf(form);
  return {
    ...kept,
    ...(efforts.length === 0 ? {} : { efforts }),
    ...(efforts.includes(form.defaultEffort) ? { default_effort: form.defaultEffort } : {}),
    ...(form.images ? { input_images: true } : {}),
    ...(form.tiers.length === 0 ? {} : { service_tiers: form.tiers }),
    ...(form.tiers.some((tier) => tier.id === form.defaultTier) ? { default_service_tier: form.defaultTier } : {}),
    ...(form.programs.length === 0 ? {} : { access_programs: form.programs }),
    ...(form.programs.includes(form.defaultProgram) ? { default_access_program: form.defaultProgram } : {}),
    ...(window === null ? {} : { context_window: window }),
    ...(largest === null ? {} : { context_max: largest }),
    // A start is only named where a larger context is open: without one, a thread works with the model's own window.
    ...(largest === null || started === null ? {} : { default_context: started }),
    ...(rate === undefined ? {} : { rate }),
  };
}

/** The member of the form that a path of a refused request is about. */
function fieldOfPath(path: string): FieldName | null {
  if (path === 'model_name') return 'routing';
  if (path === 'display_name') return 'display';
  if (path.startsWith('groups')) return 'groups';
  const member = /^parameters\.(\w+)/.exec(path)?.[1];
  switch (member) {
    case 'efforts':
      return 'efforts';
    case 'default_effort':
      return 'defaultEffort';
    case 'context_window':
      return 'window';
    case 'context_max':
      return 'largest';
    case 'default_context':
      return 'start';
    case 'default_service_tier':
    case 'service_tiers':
      return 'tier';
    case 'default_access_program':
    case 'access_programs':
      return 'program';
    case 'rate':
      return 'rate';
    default:
      return null;
  }
}

/** What the server refused, set at the fields it names; `rest` is what it said that no field is about. */
export function refusedAt(error: unknown): { faults: Faults; rest: boolean } {
  if (!(error instanceof ApiError)) return { faults: {}, rest: error !== null && error !== undefined };
  const faults: Faults = {};
  let placed = false;
  for (const fault of error.faults()) {
    const field = fieldOfPath(fault.path);
    if (field === null) continue;
    placed = true;
    faults[field] ??= fault.message;
  }
  // A refusal for a name the provider has already is about one of the two names, and the routing name comes first.
  if (error.is('conflict.exists')) {
    faults.routing ??= error.message;
    placed = true;
  }
  return { faults, rest: !placed };
}

/** What the entry has of each list that is switched on, to say at a glance. */
export function switchedOn(parameters: ModelParameters): { speeds: string[]; programs: string[]; largest: number | null } {
  return {
    speeds: (parameters.service_tiers ?? []).map((tier) => tier.label),
    programs: [...(parameters.access_programs ?? [])],
    largest: parameters.context_max ?? null,
  };
}
