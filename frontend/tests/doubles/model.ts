/** What the fake API says of what a model can be run with: the rules the server keeps to a model's entry, what accepting a suggestion makes of it, the caps of a provider, and the parameters a thread's agent publishes for speed, access and context with the views of them that a thread is shown.
 *
 * It keeps to the contract: a speed, an access program and a larger context are open to threads only when
 * the entry of the model says so, whatever the source offered; what a thread starts with is the entry's
 * own; and a thread's own choices are the object `agent.params`. The same shapes the real adapters declare
 * (`service_tier`, `cyber_access_program`, `context`) are used, so that the screens are tried against
 * what they will meet.
 */
import type { CapStatus, ModelParameters, ParameterView, SettingSource, UsageCap } from '../../src/generated/api-types.ts';
import { ACME, id as fixtureId, MADE_AT, userRef } from './fixtures.ts';
import type { Me } from '../../src/generated/api-types.ts';
import { Refusal } from './world.ts';

// ---------------------------------------------------------------- the rules of an entry

const GROUP_NAME = /^[a-z0-9][a-z0-9_-]{0,39}$/;
const GROUPS_MOST = 8;

function invalid(path: string, message: string): never {
  throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path, message }] });
}

/** What the server refuses of a model's entry: a start that is not among what is switched on, a largest context below the window, a start for the context without a largest, a group badly named. */
export function checkEntry(parameters: ModelParameters, groups: readonly string[]): void {
  for (const [member, size] of [
    ['context_window', parameters.context_window],
    ['context_max', parameters.context_max],
    ['default_context', parameters.default_context],
  ] as const) {
    if (typeof size === 'number' && size < 1) invalid(`parameters.${member}`, 'a number of tokens is at least one');
  }
  const efforts = parameters.efforts ?? [];
  if (typeof parameters.default_effort === 'string' && !efforts.includes(parameters.default_effort)) {
    invalid('parameters.default_effort', 'an effort a thread starts with is one of the efforts the model lists');
  }
  if (typeof parameters.default_service_tier === 'string' && !(parameters.service_tiers ?? []).some((tier) => tier.id === parameters.default_service_tier)) {
    invalid('parameters.default_service_tier', 'a speed a thread starts with is one of the speeds enabled on the model');
  }
  if (typeof parameters.default_access_program === 'string' && !(parameters.access_programs ?? []).includes(parameters.default_access_program)) {
    invalid('parameters.default_access_program', 'an access program a thread starts with is one of the programs enabled on the model');
  }
  if (typeof parameters.context_max === 'number' && typeof parameters.context_window === 'number' && parameters.context_max < parameters.context_window) {
    invalid('parameters.context_max', "the largest context is not below the model's own window");
  }
  if (typeof parameters.default_context === 'number') {
    if (typeof parameters.context_max !== 'number') invalid('parameters.default_context', 'a start for the context needs the largest context to be enabled');
    else if (parameters.default_context > parameters.context_max) invalid('parameters.default_context', 'the context a thread starts with is not above the largest');
  }
  groups.forEach((group, index) => {
    if (!GROUP_NAME.test(group)) invalid(`groups[${index}]`, 'a group is named by 1 to 40 of a-z, 0-9, - and _, beginning with a letter or a digit');
  });
  if (new Set(groups).size > GROUPS_MOST) invalid('groups', `a model is in at most ${GROUPS_MOST} groups`);
}

/** The entry that accepting a suggestion makes: what the suggestion lists of speeds, programs and a larger context is kept as offered, and none of it is switched on. */
export function acceptedEntry(listed: ModelParameters): ModelParameters {
  const { service_tiers, access_programs, context_max, default_service_tier: _a, default_access_program: _b, default_context: _c, offered, ...rest } = listed;
  const larger = context_max ?? offered?.context_max;
  const tiers = service_tiers ?? offered?.service_tiers;
  const programs = access_programs ?? offered?.access_programs;
  const kept = {
    ...(tiers === undefined || tiers === null ? {} : { service_tiers: tiers }),
    ...(programs === undefined || programs === null ? {} : { access_programs: programs }),
    ...(larger === undefined || larger === null || (typeof rest.context_window === 'number' && larger <= rest.context_window) ? {} : { context_max: larger }),
  };
  return { ...rest, ...(Object.keys(kept).length === 0 ? {} : { offered: kept }) };
}

// ---------------------------------------------------------------- the caps of a provider

const ALL_SCOPES = ['org', 'project', 'topic', 'thread'] as const;
const ada = (me: Me) => userRef(me);

/** Caps on a provider as the contract serves them: one for the whole provider, one for a family, one for a group, and one at a speed. */
export function capsOf(providerId: string, me: Me): CapStatus[] {
  const cap = (n: number, more: Partial<UsageCap>): CapStatus => ({
    bypasses: { count: 0, last: null },
    can_bypass: false,
    cap: {
      amount: '4',
      at_limit: 'stop',
      can_edit: true,
      counted_from: null,
      created_at: MADE_AT,
      created_by: ada(me),
      enabled: true,
      harness_kinds: [],
      id: fixtureId(0x2b, n),
      meter: 'running',
      models: [],
      node: null,
      note: '',
      period: null,
      provider: { address: 'acme.ada', harness_kind: null, harness: null, id: providerId, kind: 'api', name: 'Provider', placement: 'managed', status: 'ready' },
      revision: 1,
      split: 'shared',
      subject: { kind: 'everyone' },
      tiers: [],
      uncounted: 'allow',
      updated_at: MADE_AT,
      warn_percent: 80,
      ...more,
    },
    in_progress: '0',
    left_out: { unmetered_threads: 0, unrated_tokens: 0 },
    period_ends_at: null,
    person: null,
    replaced_by: null,
    state: 'ok',
    used: '1',
    waiting: 0,
  });
  return [
    cap(1, { amount: '4' }),
    { ...cap(2, { amount: '500', meter: 'credits', period: 'month', models: ['@frontier'] }), used: '410', state: 'warning' },
    cap(3, { amount: '2000000', meter: 'tokens', period: 'day', split: 'each', models: ['gpt-5.5*'] }),
    cap(4, { amount: '1', models: [], tiers: ['fast'] }),
  ];
}

// ---------------------------------------------------------------- models with more than the usual open

/** A rate with a multiple for the speed Fast and a step above which a prompt pays more. */
export const STEPPED_RATE = {
  input_per_mtok: '2.50',
  cached_input_per_mtok: '0.25',
  output_per_mtok: '15',
  tiers: { fast: '2' },
  above: [{ context_tokens: 272_000, input_per_mtok: '5', cached_input_per_mtok: '0.5', output_per_mtok: '22.5' }],
};

/** A model with all three open: Fast, two access programs and a context up to 1M. Run at the usual speed, with no program and all of the context unless a thread chooses. */
export const SOL_A: ModelParameters = {
  efforts: ['low', 'medium', 'high'],
  default_effort: 'medium',
  context_window: 272_000,
  context_max: 1_000_000,
  service_tiers: [{ id: 'fast', label: 'Fast' }],
  access_programs: ['Daybreak Blue', 'Daybreak Red'],
  rate: STEPPED_RATE,
};

/** A model whose managers made Fast and a context of 200K what a thread starts with; no program is open. */
export const SOL_B: ModelParameters = {
  efforts: ['low', 'high'],
  context_window: 200_000,
  context_max: 400_000,
  default_context: 200_000,
  service_tiers: [{ id: 'fast', label: 'Fast' }],
  default_service_tier: 'fast',
};

// ---------------------------------------------------------------- what the adapters publish

export const SPEED = 'service_tier';
export const ACCESS = 'cyber_access_program';
export const CONTEXT = 'context';
/** The feature of a place that switches access programs on, as the Codex adapter names it. */
export const ACCESS_FEATURE = 'codex.cyber_access';
export const CONTEXT_LEAST = 32_000;

export const SPEED_DEF = {
  key: SPEED,
  title: 'Speed',
  description: 'A faster mode of the model, which uses the plan up faster. Only the speeds a manager of the provider enabled for the model are offered.',
  group: 'model',
  type: 'enum',
  options: { from_model: 'service_tiers' },
  default: { from_model: 'default_service_tier' },
  scopes: ALL_SCOPES,
  apply: 'next_turn',
  eligible_when: { model_has: 'service_tiers' },
} as const;

export const CONTEXT_DEF = {
  key: CONTEXT,
  title: 'Context',
  description:
    "How much of a conversation the agent keeps, in tokens, up to the largest the model's managers opened. Codex shortens the conversation before it reaches that size, keeping room for the answer and for what it adds to every request: at about 828 000 for a context of 1 000 000.",
  group: 'model',
  type: 'integer',
  range: { min: CONTEXT_LEAST, max: { from_model: 'context_max' } },
  default: { from_model: 'default_context' },
  scopes: ALL_SCOPES,
  apply: 'new_session',
  eligible_when: { model_has: 'context_max' },
} as const;

export const ACCESS_DEF = {
  key: ACCESS,
  title: 'Access program',
  description: 'An authorised program for security work, which the vendor grants to the account. Only the programs a manager of the provider enabled for the model are offered.',
  group: 'safety',
  type: 'enum',
  options: { from_model: 'access_programs' },
  default: { from_model: 'default_access_program' },
  scopes: ['thread'],
  apply: 'next_turn',
  eligible_when: { all: [{ credential_path: 'native' }, { model_has: 'access_programs' }] },
  feature: ACCESS_FEATURE,
} as const;

/** What the fake knows of a thread's own choices and of the place it is in, to draw the parameters of its agent. */
export interface ThreadChoices {
  own: Record<string, unknown>;
  /** The features of the place (`agent.features`). */
  features: Record<string, boolean>;
  writable: boolean;
  /** Where the thread's choices come from, for a lock above it. */
  lockedBy?: { scope: 'org' | 'project' | 'topic'; name: string } | null;
}

const SOURCE_DEFAULT: SettingSource = { scope: 'default', node: null };

/** The parameters of the thread's agent that exist for its model, each with the thread's own choice. */
export function parameterViews(model: ModelParameters, choices: ThreadChoices): ParameterView[] {
  const source: SettingSource = Object.keys(choices.own).length > 0 ? { scope: 'thread', node: null } : SOURCE_DEFAULT;
  const lockedBy = choices.lockedBy === undefined || choices.lockedBy === null ? null : { scope: choices.lockedBy.scope, node: { id: ACME.org, kind: 'org' as const, name: choices.lockedBy.name, redacted: false } };
  const view = (def: { key: string; apply: ParameterView['apply'] }): ParameterView => ({
    key: def.key,
    def,
    value: choices.own[def.key] ?? null,
    source,
    locked_by: lockedBy,
    writable: choices.writable && lockedBy === null,
    apply: def.apply,
  });
  const views: ParameterView[] = [];
  if ((model.service_tiers ?? []).length > 0) views.push(view(SPEED_DEF));
  if ((model.access_programs ?? []).length > 0 && choices.features[ACCESS_FEATURE] === true) views.push(view(ACCESS_DEF));
  if (typeof model.context_max === 'number' && model.context_max > 0) views.push(view(CONTEXT_DEF));
  return views;
}
