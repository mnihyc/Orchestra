/** What a model can be run with and what a thread runs with: the entry as a form and the rules the server keeps to it, how a pattern names models, the sizes of a context, and the speed, access and context of a thread worked out from the entry and from what the thread chose. */
import type { TFunction } from 'i18next';
import { describe, expect, it } from 'vitest';

import { ApiError } from '@/api/client';
import { chooserOf, contextStart, droppedFor, kindOf, programStart, readDef, speedStart, type State, statesOf, withChoice, withoutDropped } from '@/features/composer/modelState';
import { ranWithParts } from '@/features/composer/RanWith';
import { readSize, readSlash } from '@/features/composer/SlashMenu';
import { contextChoices, firstStep, formatContext, readTokens, stepBelow } from '@/features/models/context';
import { groupPattern, isGroupName, listNames, patternNames } from '@/features/models/patterns';
import { faultsOf, formOf, parametersOf, refusedAt } from '@/features/resources/providers/entry';
import type { ModelParameters, Problem, ThreadParameters } from '@/generated/api-types';
import { setLocale } from '@/i18n';

/** A translation that says its key and what it was given, so that a test sees which sentence was chosen. */
const t = ((key: string, options?: Record<string, unknown>) => (options === undefined ? key : `${key} ${JSON.stringify(options)}`)) as unknown as TFunction;

/** What a source offers for a model, none of it switched on. */
const OFFERED_ONLY: ModelParameters = {
  efforts: ['low', 'high'],
  context_window: 272_000,
  offered: { service_tiers: [{ id: 'fast', label: 'Fast' }], access_programs: ['Daybreak Blue'], context_max: 1_000_000 },
};

describe("a model's entry as a form", () => {
  it('keeps what the source offered and switches nothing on by itself', () => {
    const saved = parametersOf(formOf(OFFERED_ONLY), OFFERED_ONLY);
    expect(saved.offered).toEqual(OFFERED_ONLY.offered);
    expect(saved.service_tiers).toBeUndefined();
    expect(saved.access_programs).toBeUndefined();
    expect(saved.context_max).toBeUndefined();
  });

  it('writes a speed, a program and a larger context once a manager has switched each on, with what threads start with', () => {
    const form = {
      ...formOf(OFFERED_ONLY),
      tiers: [{ id: 'fast', label: 'Fast' }],
      defaultTier: 'fast',
      programs: ['Daybreak Blue'],
      defaultProgram: 'Daybreak Blue',
      largest: '1000000',
      start: 'window' as const,
    };
    expect(parametersOf(form, OFFERED_ONLY)).toMatchObject({
      service_tiers: [{ id: 'fast', label: 'Fast' }],
      default_service_tier: 'fast',
      access_programs: ['Daybreak Blue'],
      default_access_program: 'Daybreak Blue',
      context_window: 272_000,
      context_max: 1_000_000,
      default_context: 272_000,
      offered: OFFERED_ONLY.offered,
    });
  });

  it('starts threads with all of it unless a manager chose another start, and names no start without a largest context', () => {
    const open = { ...formOf(OFFERED_ONLY), largest: '1000000' };
    expect(open.start).toBe('all');
    expect(parametersOf(open, OFFERED_ONLY).default_context).toBeUndefined();
    expect(parametersOf({ ...open, start: 'number', startNumber: '400000' }, OFFERED_ONLY).default_context).toBe(400_000);
    // The model's own window is the start only where a larger context is open.
    expect(parametersOf({ ...formOf(OFFERED_ONLY), start: 'window' }, OFFERED_ONLY).default_context).toBeUndefined();
  });

  it('reads a start that equals the model\'s own window as the window, and any other number as a number', () => {
    expect(formOf({ context_window: 272_000, context_max: 1_000_000, default_context: 272_000 }).start).toBe('window');
    expect(formOf({ context_window: 272_000, context_max: 1_000_000, default_context: 400_000 })).toMatchObject({ start: 'number', startNumber: '400000' });
    expect(formOf({ context_window: 272_000, context_max: 1_000_000 }).start).toBe('all');
  });

  it('refuses a largest context below the model\'s own window, and a start above the largest', () => {
    const form = { ...formOf({ context_window: 272_000 }), largest: '200000' };
    expect(faultsOf(form, t).largest).toContain('models.fault.largestBelowWindow');
    const above = { ...formOf({ context_window: 272_000 }), largest: '400000', start: 'number' as const, startNumber: '500000' };
    expect(faultsOf(above, t).start).toContain('models.fault.startAboveLargest');
    expect(faultsOf({ ...above, startNumber: '400000' }, t)).toEqual({});
  });

  it('refuses a size that is no whole number, and a group badly named or more than eight', () => {
    expect(faultsOf({ ...formOf({}), window: '2.5' }, t).window).toBe('models.fault.size');
    expect(faultsOf({ ...formOf({}), groups: ['Frontier'] }, t).groups).toContain('models.fault.groupName');
    expect(faultsOf({ ...formOf({}), groups: Array.from({ length: 9 }, (_, n) => `g${n}`) }, t).groups).toContain('models.fault.groupsMost');
    expect(faultsOf({ ...formOf({}), groups: ['frontier', 'small-fast', 'v_2'] }, t)).toEqual({});
  });

  it('keeps the steps of a rate for large prompts and the multiple of a speed', () => {
    const rate = { input_per_mtok: '2.5', cached_input_per_mtok: '0.25', output_per_mtok: '15', tiers: { fast: '2' }, above: [{ context_tokens: 272_000, input_per_mtok: '5', cached_input_per_mtok: '0.5', output_per_mtok: '22.5' }] };
    const entry: ModelParameters = { rate, service_tiers: [{ id: 'fast', label: 'Fast' }] };
    expect(parametersOf(formOf(entry), entry).rate).toEqual(rate);
    expect(faultsOf({ ...formOf(entry), steps: [{ over: '', input: '', cached: '', output: '', write: '' }] }, t).steps).toBe('models.fault.stepIncomplete');
  });

  it('places what the server refused at the field it names', () => {
    const problem = (errors: { path: string; message: string }[]): Problem => ({ type: 'x', code: 'request.invalid', title: 't', status: 400, detail: 'd', request_id: 'r', errors }) as unknown as Problem;
    const refused = refusedAt(new ApiError(problem([{ path: 'parameters.context_max', message: 'the largest context is not below the model\'s own window' }, { path: 'groups[1]', message: 'a group is named by' }]), null));
    expect(refused.faults.largest).toContain('largest context');
    expect(refused.faults.groups).toContain('group');
    expect(refused.rest).toBe(false);
    expect(refusedAt(new ApiError(problem([{ path: 'something_else', message: 'm' }]), null)).rest).toBe(true);
  });
});

describe('how a pattern names models', () => {
  it('covers a family through any gateway, and a route only when it says so', () => {
    expect(patternNames('gpt-6*', 'gpt-6-sol', [])).toBe(true);
    expect(patternNames('gpt-6*', 'openai/gpt-6-sol', [])).toBe(true);
    expect(patternNames('openai/*', 'openai/gpt-6-sol', [])).toBe(true);
    expect(patternNames('openai/*', 'azure/gpt-6-sol', [])).toBe(false);
    expect(patternNames('GPT-6-SOL', 'gpt-6-sol', [])).toBe(true);
    expect(patternNames('gpt-6', 'gpt-6-sol', [])).toBe(false);
  });

  it('names a model by a group it is in, and a group no model is in names nothing', () => {
    expect(patternNames(groupPattern('frontier'), 'gpt-6-sol', ['frontier'])).toBe(true);
    expect(patternNames('@frontier', 'gpt-6-sol', ['small'])).toBe(false);
    expect(patternNames('@frontier', 'frontier', [])).toBe(false);
    expect(listNames(['claude-*', '@frontier'], 'openai/gpt-6-sol', ['frontier'])).toBe(true);
    expect(listNames([], 'gpt-6-sol', ['frontier'])).toBe(false);
  });

  it('names a group by letters, digits, - and _ beginning with a letter or a digit', () => {
    expect(['frontier', 'a', '1x', 'small_fast-2'].every(isGroupName)).toBe(true);
    expect(['', 'Frontier', '-x', '_x', 'two words', 'x'.repeat(41)].some(isGroupName)).toBe(false);
  });
});

describe('a context in tokens', () => {
  it('is written as people say it', () => {
    expect(formatContext(1_000_000, 'en-US')).toBe('1M');
    expect(formatContext(400_000, 'en-US')).toBe('400K');
    expect(formatContext(828_400, 'en-US')).toBe('828.4K');
    expect(formatContext(1_050_000, 'en-US')).toBe('1.05M');
    expect(formatContext(950, 'en-US')).toBe('950');
  });

  it('reads a number the way a person writes a long one', () => {
    expect(readTokens('1 000 000')).toBe(1_000_000);
    expect(readTokens('272,000')).toBe(272_000);
    expect(readTokens('2.5')).toBeNull();
    expect(readTokens('')).toBeNull();
  });

  it('offers the largest, the model\'s own window, and a few round sizes between them, the largest first', () => {
    const sizes = contextChoices({ least: 32_000, own: 272_000, largest: 1_000_000 });
    expect(sizes[0]).toBe(1_000_000);
    expect(sizes.at(-1)).toBe(272_000);
    expect(sizes.length).toBeLessThanOrEqual(5);
    expect([...sizes].sort((one, other) => other - one)).toEqual(sizes);
    // Nothing is offered below the window or the least.
    expect(sizes.every((size) => size >= 272_000)).toBe(true);
    expect(contextChoices({ least: 32_000, own: null, largest: 128_000 }).every((size) => size >= 32_000)).toBe(true);
  });

  it('says which step of a rate a size lies above', () => {
    const rate = { above: [{ context_tokens: 272_000, input_per_mtok: '5', output_per_mtok: '22' }, { context_tokens: 600_000, input_per_mtok: '8', output_per_mtok: '30' }] };
    expect(stepBelow(rate, 200_000)).toBeNull();
    expect(stepBelow(rate, 272_000)).toBeNull();
    expect(stepBelow(rate, 400_000)).toBe(272_000);
    expect(stepBelow(rate, 1_000_000)).toBe(600_000);
    expect(firstStep(rate)).toBe(272_000);
    expect(stepBelow(null, 1_000_000)).toBeNull();
  });
});

// ---------------------------------------------------------------- what a thread runs with

const SPEED = { key: 'service_tier', title: 'Speed', description: 'A faster mode.', type: 'enum', options: { from_model: 'service_tiers' }, default: { from_model: 'default_service_tier' }, apply: 'next_turn' };
const ACCESS = { key: 'cyber_access_program', title: 'Access program', description: 'A program.', type: 'enum', options: { from_model: 'access_programs' }, default: { from_model: 'default_access_program' }, apply: 'next_turn' };
const CONTEXT = { key: 'context', title: 'Context', description: 'Where it is shortened.', type: 'integer', range: { min: 32_000, max: { from_model: 'context_max' } }, default: { from_model: 'default_context' }, apply: 'new_session' };

/** A model with each of the three open. */
const SOL: ModelParameters = {
  context_window: 272_000,
  context_max: 1_000_000,
  service_tiers: [{ id: 'fast', label: 'Fast' }],
  access_programs: ['Daybreak Blue'],
};

const parametersFor = (model: ModelParameters, views: { def: unknown; value?: unknown; scope?: 'thread' | 'default' }[]): ThreadParameters =>
  ({
    effort: { value: null, source: { scope: 'default', node: null }, writable: true, supported: true },
    model: { value: 'sol', source: { scope: 'default', node: null }, writable: true, options: [{ allowed: true, display_name: 'Sol', model_name: 'sol', parameters: model, reason: null }] },
    parameters: views.map((view) => ({
      key: (view.def as { key: string }).key,
      def: view.def,
      value: view.value ?? null,
      source: { scope: view.scope ?? 'default', node: null },
      locked_by: null,
      writable: true,
      apply: (view.def as { apply: string }).apply,
    })),
  }) as unknown as ThreadParameters;

const all = (model: ModelParameters, own: Record<string, unknown> = {}): State[] =>
  statesOf(parametersFor(model, [{ def: SPEED, value: own.service_tier, scope: own.service_tier === undefined ? 'default' : 'thread' }, { def: ACCESS, value: own.cyber_access_program }, { def: CONTEXT, value: own.context }]), own);

describe('the parameters of a thread\'s agent', () => {
  it('are told apart by what their definition says and never by their key', () => {
    expect(kindOf(readDef(SPEED) ?? { key: '', title: '', description: '', options: null, range: null })).toBe('speed');
    expect(kindOf(readDef(ACCESS)!)).toBe('access');
    expect(kindOf(readDef(CONTEXT)!)).toBe('context');
    // Another adapter's keys, the same meaning.
    expect(kindOf(readDef({ ...SPEED, key: 'fast_mode' })!)).toBe('speed');
    expect(kindOf(readDef({ key: 'verbosity', options: [{ value: 'terse', label: 'Terse' }] })!)).toBeNull();
    expect(readDef({ title: 'no key' })).toBeNull();
  });
});

describe('what a model starts with', () => {
  it('is the ordinary speed and no program until a manager named another, and a named one only while it is still switched on', () => {
    expect(speedStart(SOL)).toBeNull();
    expect(programStart(SOL)).toBeNull();
    expect(speedStart({ ...SOL, default_service_tier: 'fast' })).toBe('fast');
    expect(speedStart({ ...SOL, service_tiers: [], default_service_tier: 'fast' })).toBeNull();
    expect(programStart({ ...SOL, default_access_program: 'Daybreak Blue' })).toBe('Daybreak Blue');
  });

  it('is all of the context that is open unless a manager named another start, and its own window where nothing larger is open', () => {
    expect(contextStart(SOL)).toBe(1_000_000);
    expect(contextStart({ ...SOL, default_context: 272_000 })).toBe(272_000);
    expect(contextStart({ ...SOL, default_context: 2_000_000 })).toBe(1_000_000);
    expect(contextStart({ context_window: 272_000 })).toBeNull();
  });
});

describe('what a thread runs with', () => {
  it('opens on what the model starts with when nothing was chosen, and says so', () => {
    const [speed, access, context] = all({ ...SOL, default_service_tier: 'fast' });
    expect(speed).toMatchObject({ kind: 'speed', force: 'fast', chosen: null, from: 'start' });
    expect(access).toMatchObject({ kind: 'access', force: null, chosen: null, from: 'start' });
    expect(context).toMatchObject({ kind: 'context', force: 1_000_000, chosen: null, from: 'start', largest: 1_000_000, window: 272_000 });
  });

  it('shows a speed that is switched on and not chosen as the ordinary one, with Fast as a choice', () => {
    const [speed] = all(SOL);
    expect(speed).toMatchObject({ kind: 'speed', force: null, start: null, open: [{ id: 'fast', label: 'Fast' }] });
  });

  it('runs with a choice that the entry still opens, as the thread\'s own', () => {
    const [speed, access, context] = all(SOL, { service_tier: 'fast', cyber_access_program: 'Daybreak Blue', context: 400_000 });
    expect(speed).toMatchObject({ force: 'fast', chosen: 'fast', from: 'chosen', stale: false });
    expect(access).toMatchObject({ force: 'Daybreak Blue', from: 'chosen' });
    expect(context).toMatchObject({ force: 400_000, chosen: 400_000, from: 'chosen' });
  });

  it('flags a choice that is no longer open and runs with what the model starts with instead', () => {
    const [speed, , context] = all(SOL, { service_tier: 'priority', context: 2_000_000 });
    expect(speed).toMatchObject({ force: null, chosen: null, stale: true, from: 'start' });
    expect(context).toMatchObject({ force: 1_000_000, chosen: null, stale: true });
    // Below what the adapter takes is as little open as above what the model opens.
    expect(all(SOL, { context: 1_000 })[2]).toMatchObject({ stale: true, force: 1_000_000 });
  });

  it('keeps the ordinary speed and no program as choices of their own where the model starts with one', () => {
    const model = { ...SOL, default_service_tier: 'fast', default_access_program: 'Daybreak Blue' };
    const [speed, access] = all(model, { service_tier: null, cyber_access_program: null });
    expect(speed).toMatchObject({ force: null, chosen: null, from: 'chosen', stale: false });
    expect(access).toMatchObject({ force: null, from: 'chosen' });
    expect(withChoice({}, speed!, null)).toEqual({ service_tier: null });
    expect(withChoice({ service_tier: null }, speed!, 'fast')).toEqual({});
  });

  it('has no speed, access or context for a model whose managers switched none on', () => {
    expect(statesOf(parametersFor({ efforts: ['low'] }, []), {})).toEqual([]);
  });
});

describe('a change of model', () => {
  const states = all(SOL, { service_tier: 'fast', cyber_access_program: 'Daybreak Blue', context: 800_000 });
  const own = { service_tier: 'fast', cyber_access_program: 'Daybreak Blue', context: 800_000 };

  it('drops every choice that a model with nothing open does not have, and never carries one over', () => {
    const dropped = droppedFor(states, own, { efforts: ['low'] });
    expect(dropped.map((one) => [one.kind, one.was, one.becomes])).toEqual([
      ['speed', 'Fast', null],
      ['access', 'Daybreak Blue', null],
      ['context', 800_000, null],
    ]);
    expect(withoutDropped(own, dropped)).toEqual({});
  });

  it('keeps a context that fits the new model and drops one that does not, falling back to that model\'s start', () => {
    const next: ModelParameters = { service_tiers: [{ id: 'fast', label: 'Fast' }], context_window: 200_000, context_max: 400_000, default_context: 200_000 };
    const fits = droppedFor(all(SOL, { context: 300_000 }), { context: 300_000 }, next);
    expect(fits).toEqual([]);
    const over = droppedFor(states, own, next);
    expect(over.map((one) => one.kind)).toEqual(['access', 'context']);
    expect(over.find((one) => one.kind === 'context')?.becomes).toBe(200_000);
    expect(withoutDropped(own, over)).toEqual({ service_tier: 'fast' });
  });

  it('leaves alone a thread that chose nothing', () => {
    expect(droppedFor(all(SOL), {}, { efforts: ['low'] })).toEqual([]);
  });
});

describe('a choice', () => {
  const [speed, , context] = all(SOL, { service_tier: 'fast' });

  it('is written as the thread\'s own, and choosing what the model starts with puts the thread back to following it', () => {
    expect(withChoice({}, speed!, 'fast')).toEqual({ service_tier: 'fast' });
    expect(withChoice({ service_tier: 'fast', other: 1 }, speed!, null)).toEqual({ other: 1 });
    expect(withChoice({}, context!, 400_000)).toEqual({ context: 400_000 });
    expect(withChoice({ context: 400_000 }, context!, 1_000_000)).toEqual({});
  });

  it('is credited to whoever last changed it, as far as the lines of the history say', () => {
    const lines = [
      { key: 'agent.params', from: {}, to: { service_tier: 'fast' }, by: { display_name: 'Dana' } },
      { key: 'agent.params', from: { service_tier: 'fast' }, to: { service_tier: 'fast', context: 400_000 }, by: { display_name: 'Bo' } },
    ];
    expect(chooserOf(lines, 'service_tier')).toEqual({ name: 'Dana' });
    expect(chooserOf(lines, 'context')).toEqual({ name: 'Bo' });
    expect(chooserOf(lines, 'cyber_access_program')).toBeNull();
    expect(chooserOf([{ key: 'context', from: null, to: 400_000, by: { display_name: 'Eve' } }], 'context')).toEqual({ name: 'Eve' });
  });
});

describe('what is written in the box after a slash', () => {
  it('is a command being typed, one with its argument, or no command at all', () => {
    expect(readSlash('/')).toEqual({ name: '', argument: '', typing: true });
    expect(readSlash('/Sp')).toEqual({ name: 'sp', argument: '', typing: true });
    expect(readSlash('/speed fast')).toEqual({ name: 'speed', argument: 'fast', typing: false });
    expect(readSlash('/access daybreak blue ')).toEqual({ name: 'access', argument: 'daybreak blue', typing: false });
    // A path, a message and text over several lines are no command.
    expect(readSlash('/etc/hosts is wrong')).toBeNull();
    expect(readSlash('hello /speed')).toBeNull();
    expect(readSlash('/speed fast\nand more')).toBeNull();
  });

  it('writes a size as people do: 400k, 1M, 1.5m, or the whole number', () => {
    expect(readSize('400k')).toBe(400_000);
    expect(readSize('1M')).toBe(1_000_000);
    expect(readSize('1.5m')).toBe(1_500_000);
    expect(readSize('272 000')).toBe(272_000);
    expect(readSize('400000')).toBe(400_000);
    expect(readSize('')).toBeNull();
    expect(readSize('fast')).toBeNull();
    expect(readSize('0')).toBeNull();
  });
});

describe("what a turn ran with, as its footer says it", () => {
  const defs = [readDef(SPEED), readDef(ACCESS), readDef(CONTEXT), readDef({ key: 'verbosity', options: [{ value: 'terse', label: 'Terse' }] })].flatMap((def) => (def === null ? [] : [def]));
  const options = [{ model_name: 'sol', parameters: SOL }];

  it('names the speed, the program and the context it was started with, in that order', () => {
    expect(ranWithParts({ context: 400_000, cyber_access_program: 'Daybreak Blue', service_tier: 'fast', context_window: 272_000 }, defs, 'sol', options, 'en-US')).toEqual(['Fast', 'Daybreak Blue', '400K']);
  });

  it('names recorded access programs in the reader’s language and retains unknown ids', async () => {
    expect(ranWithParts({ cyber_access_program: 'daybreakBlue' }, defs, 'sol', options, 'en')).toEqual(['Daybreak Blue']);
    await setLocale('zh-CN');
    expect(ranWithParts({ cyber_access_program: 'standard' }, defs, 'sol', options, 'zh-CN')).toEqual(['标准']);
    await setLocale('en');
    expect(ranWithParts({ cyber_access_program: 'daybreakGreen' }, defs, 'sol', options, 'en')).toEqual(['daybreakGreen']);
  });

  it('says nothing of what it did not record, and nothing that no definition tells apart', () => {
    expect(ranWithParts({}, defs, 'sol', options, 'en-US')).toEqual([]);
    expect(ranWithParts({ context: 1_000_000 }, defs, 'sol', options, 'en-US')).toEqual(['1M']);
    expect(ranWithParts({ verbosity: 'terse', mystery: 'x' }, defs, 'sol', options, 'en-US')).toEqual([]);
    expect(ranWithParts(null, defs, 'sol', options, 'en-US')).toEqual([]);
  });

  it('writes a speed that no entry names as it was recorded', () => {
    expect(ranWithParts({ service_tier: 'priority' }, defs, 'sol', options, 'en-US')).toEqual(['priority']);
    expect(ranWithParts({ service_tier: 'fast' }, defs, 'gone', options, 'en-US')).toEqual(['fast']);
  });
});
