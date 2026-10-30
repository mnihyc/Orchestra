/** What the usage screens read out of what the server says and write back into it: the span an address names and how it is cut, how credits and tokens are written, what a chart is drawn from, a cap in words, and a cap that is made and changed. */
import type { TFunction } from 'i18next';
import { describe, expect, it, vi } from 'vitest';

import type { CapStatus, UsageAnalytics, UsageCap, UsageFigures, UsageKey } from '@/generated/api-types';
import { amountText, check, draftOf, emptyDraft, inputOf, isChange, modelPattern, splitOf, subjectOf, updateOf } from '@/features/usage/capForm';
import { changeOf, creditsState, formatCredits, formatMeasure, tokenParts } from '@/features/usage/figures';
import { filterValue, isRest, keyDetail, keyId, keyLabel } from '@/features/usage/keys';
import { assignSlots, barsOf, REST_SLOT } from '@/features/usage/series';
import {
  bucketAsked,
  bucketsFor,
  defaultBucket,
  figuresParams,
  filtersOf,
  offersThreads,
  parseScope,
  patchSearch,
  readSearch,
  scopeText,
  spanBefore,
  spanBetween,
  spanOf,
  tablesFor,
} from '@/features/usage/search';
import { tightestPercent } from '@/features/usage/ThreadUsage';
import { allowanceWords, capSentence, capTitle, dayWords, startsAgain, usedWords } from '@/features/usage/words';
import { i18n } from '@/i18n';
import { programName } from '@/features/resources/programNames';
import { DETAIL_PAGE_ROWS } from '@/features/resources/DetailSections';

const t = i18n.t.bind(i18n) as unknown as TFunction;

/** A Monday at noon, UTC: the 12th of October 2026. */
const NOW = Date.parse('2026-10-12T12:00:00Z');
const ID = '00000000-0000-4000-8003-000000000002';

const figures = (turns: number, total: number, credits: string, over: Partial<UsageFigures> = {}): UsageFigures => ({
  turns,
  tokens: { input_tokens: total, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: total },
  credits,
  estimated_credits: '0',
  unrated_tokens: 0,
  ...over,
});

describe('the address of a usage screen', () => {
  it('says nothing for what is the default: the thirty days, in credits, split by model', () => {
    expect(readSearch({ range: '30d', show: 'credits', series: 'model' })).toEqual({range:'30d',show:'credits',series:'model'});
    expect(readSearch({})).toEqual({});
  });

  it('keeps what it can read and drops what it cannot', () => {
    expect(readSearch({ scope: 'node:not-an-id', range: 'forever', from: '2026-13-45', show: 'dollars', person: 'ada', more: 'yes' })).toEqual({});
    expect(readSearch({ scope: `node:${ID}`, range: '7d', show: 'tokens', series: 'none', by: 'model', model: 'gpt-5.4', environment: 'managed', more: true })).toEqual({
      scope: `node:${ID}`,
      range: '7d',
      show: 'tokens',
      series: 'none',
      by: 'model',
      model: 'gpt-5.4',
      environment: 'managed',
      more: true,
    });
  });

  it('names one scope for one person, a place or a provider', () => {
    expect(parseScope(`node:${ID}`)).toEqual({ kind: 'node', id: ID });
    expect(parseScope(`provider:${ID}`)).toEqual({ kind: 'provider', id: ID });
    expect(parseScope('me')).toEqual({ kind: 'me' });
    expect(parseScope('place:1')).toEqual({ kind: 'me' });
    expect(scopeText({ kind: 'node', id: ID })).toBe(`node:${ID}`);
  });

  it('lets a span of one\'s own win over a named one, and a named one win over it again', () => {
    const own = patchSearch({ range: '7d' }, { from: '2026-10-01', to: '2026-10-05' });
    expect(own).toEqual({ from: '2026-10-01', to: '2026-10-05' });
    expect(patchSearch(own, { range: '90d' })).toEqual({ range: '90d' });
    expect(patchSearch({ range: '90d' }, { range: '30d' })).toEqual({range:'30d'});
  });
});

describe('the span an address names', () => {
  it('keeps Last week to the previous Monday–Sunday across week and year boundaries', () => {
    for(const at of ['2026-10-12T00:00:00Z','2026-10-18T23:59:59Z']) {
      expect(spanOf(readSearch({range:'last_week'}),Date.parse(at))).toEqual({from:'2026-10-05',to:'2026-10-11',days:7,range:'last_week'});
    }
    expect(spanOf({range:'last_week'},Date.parse('2027-01-01T12:00:00Z'))).toEqual({from:'2026-12-21',to:'2026-12-27',days:7,range:'last_week'});
  });

  it('counts the days of each span in UTC, ending today', () => {
    expect(spanOf({ range: '7d' }, NOW)).toMatchObject({ from: '2026-10-06', to: '2026-10-12', days: 7, range: '7d' });
    expect(spanOf({}, NOW)).toMatchObject({ from: '2026-09-13', to: '2026-10-12', days: 30, range: '30d' });
    expect(spanOf({ range: '90d' }, NOW).days).toBe(90);
    expect(spanOf({ range: 'year' }, NOW)).toMatchObject({ from: '2026-01-01', to: '2026-10-12', days: 285 });
    expect(spanOf({ range: 'all' }, NOW).from).toBe('2020-01-01');
  });

  it('reads two days that are the wrong way round the right way round', () => {
    expect(spanOf({ from: '2026-10-10', to: '2026-10-01' }, NOW)).toMatchObject({ from: '2026-10-01', to: '2026-10-10', days: 10, range: 'custom' });
  });

  it('puts the span before at the same length, ending the day before; all time has nothing before it', () => {
    const before = spanBefore(spanOf({ range: '7d' }, NOW));
    expect(before).toMatchObject({ from: '2026-09-29', to: '2026-10-05', days: 7 });
    expect(spanBefore(spanOf({ range: 'all' }, NOW))).toBeNull();
  });

  it('turns days dragged across the chart into the days they cover', () => {
    expect(spanBetween(Date.parse('2026-10-03T14:00:00Z'), Date.parse('2026-10-05T01:00:00Z'))).toEqual({ from: '2026-10-03', to: '2026-10-05' });
    expect(spanBetween(Date.parse('2026-10-05T01:00:00Z'), Date.parse('2026-10-03T14:00:00Z'))).toEqual({ from: '2026-10-03', to: '2026-10-05' });
  });
});

describe('how a span is cut', () => {
  it('chooses by the length: hours to two days, days to 92, weeks to two years, then months', () => {
    expect([1, 2, 3, 92, 93, 730, 731].map(defaultBucket)).toEqual(['hour', 'hour', 'day', 'day', 'week', 'week', 'month']);
  });

  it('lets by the hour be asked for a month at most and by the day for 800 days', () => {
    expect(bucketsFor(31)).toEqual(['hour', 'day', 'week', 'month']);
    expect(bucketsFor(32)).toEqual(['day', 'week', 'month']);
    expect(bucketsFor(801)).toEqual(['week', 'month']);
  });

  it('asks for none that the span does not allow, and leaves the choice to the server', () => {
    expect(bucketAsked({ bucket: 'hour' }, 90)).toBeUndefined();
    expect(bucketAsked({ bucket: 'day' }, 365)).toBe('day');
    expect(bucketAsked({}, 365)).toBeUndefined();
  });

  it('asks for a year by the day when a person picks it', () => {
    const search = readSearch({ range: 'year', bucket: 'day' });
    const params = figuresParams({ kind: 'me' }, search, spanOf(search, NOW), ['model'], 10);
    expect(params).toMatchObject({ scope: 'me', from: '2026-01-01', to: '2026-10-12', bucket: 'day', seriesBy: 'model', breakdowns: ['model'], limit: 10 });
  });
});

describe('what the whole screen is narrowed to', () => {
  it('goes into the request by the member the API knows each by', () => {
    expect(filtersOf({ person: ID, model: 'gpt-5.4', provider: ID, agent: 'codex', environment: 'connected', project: ID, thread: ID })).toEqual({
      user_id: ID,
      model: 'gpt-5.4',
      provider_id: ID,
      harness_kind: 'codex',
      environment_kind: 'connected',
      project_id: ID,
      thread_id: ID,
    });
  });

  it('offers the tables that make sense for what is asked about: the people only where there are others, the orgs for a provider', () => {
    expect(tablesFor({ kind: 'me' }, null)).not.toContain('person');
    expect(tablesFor({ kind: 'provider', id: ID }, null)).toContain('org');
    expect(tablesFor({ kind: 'node', id: ID }, 'topic')).not.toContain('project');
    expect(tablesFor({ kind: 'node', id: ID }, 'org')).toEqual(expect.arrayContaining(['person', 'org', 'project']));
  });

  it('gives the tables by thread to the reader\'s own use and to reports, and not to a summary', () => {
    expect(offersThreads({ kind: 'node', id: ID }, 'summary')).toBe(false);
    expect(offersThreads({ kind: 'node', id: ID }, 'reports')).toBe(true);
    expect(offersThreads({ kind: 'me' }, 'own')).toBe(true);
  });
});

describe('credits and tokens', () => {
  it('are written with the digits that mean something: one decimal from ten, two below, and a fraction to two figures', () => {
    expect(formatCredits(0, 'en')).toBe('0');
    expect(formatCredits(412.64, 'en')).toBe('412.6');
    expect(formatCredits(2.1, 'en')).toBe('2.1');
    expect(formatCredits(0.4, 'en')).toBe('0.4');
    expect(formatCredits(0.003552, 'en')).toBe('0.0036');
    expect(formatCredits(0.0004, 'en')).toBe('<0.001');
    expect(formatCredits(1234.56, 'de')).toBe('1.234,6');
    expect(formatMeasure(86_200_000, 'tokens', 'en')).toBe('86.2M');
    expect(formatMeasure(1_204, 'turns', 'en')).toBe('1,204');
  });

  it('say how much of a sum is an estimate and how much use it leaves out, and read nothing but "–" for use that had no rate', () => {
    const mixed = creditsState(figures(10, 1_000, '412.6', { estimated_credits: '96', unrated_tokens: 4_100_000 }));
    expect(mixed).toMatchObject({ sum: 412.6, estimated: 96, unrated: 4_100_000, allEstimated: false, none: false });
    expect(creditsState(figures(3, 300, '12', { estimated_credits: '12' })).allEstimated).toBe(true);
    expect(creditsState(figures(3, 300, '0', { unrated_tokens: 300 })).none).toBe(true);
    // Nothing used is not use without a rate: zero is written where something was counted and came to nothing.
    expect(creditsState(figures(0, 0, '0')).none).toBe(false);
  });

  it('are compared with the span before: up, down, the same, new, or nothing to compare', () => {
    expect(changeOf(112, 100)).toEqual({ kind: 'up', percent: 12 });
    expect(changeOf(88, 100)).toEqual({ kind: 'down', percent: 12 });
    expect(changeOf(100.2, 100)).toEqual({ kind: 'same' });
    expect(changeOf(5, 0)).toEqual({ kind: 'new' });
    expect(changeOf(0, 0)).toEqual({ kind: 'same' });
    expect(changeOf(5, null)).toEqual({ kind: 'none' });
  });

  it('divide the tokens the way a rate does: input not cached, cached input, output of which some was reasoning', () => {
    expect(tokenParts({ input_tokens: 41_000_000, cached_input_tokens: 31_200_000, output_tokens: 14_000_000, reasoning_tokens: 6_200_000, total_tokens: 55_000_000 })).toEqual({
      input: 9_800_000,
      cached: 31_200_000,
      output: 14_000_000,
      reasoning: 6_200_000,
    });
  });
});

describe('what a row of a table is about', () => {
  const person: UsageKey = { person: { id: ID, kind: 'human', username: 'bo', display_name: 'Bo Lindqvist', avatar: { text: 'B', color: 1 } } };

  it('is a person or model by its display name, never a routing subtitle, and use of nobody as such', () => {
    expect(keyLabel('person', person, t)).toBe('Bo Lindqvist');
    expect(keyLabel('person', { person: null }, t)).toBe('Not put down to a person');
    expect(keyLabel('model', { model: 'gpt-5.4', model_display: 'GPT-5.4' }, t)).toBe('GPT-5.4');
    expect(keyLabel('model', { model: 'our-local-model', model_display: '' }, t)).toBe(t('composer.model.displayUnknown'));
    expect(keyDetail('model', { model: 'private_model_route', model_display: 'Reasoner' }, t)).toBeNull();
    expect(filterValue('model', { model: 'private_model_route', model_display: 'Reasoner' })).toBe('private_model_route');
    expect(keyLabel('environment', { environment: 'managed' }, t)).toBe("On Orch's hosts");
    expect(keyLabel('environment', { environment: 'connected' }, t)).toBe("On people's machines");
    expect(keyLabel('tier', { tier: null }, t)).toBe('Standard');
  });

  it('is a line without a name where the reader may not see the place, and cannot be pressed', () => {
    const hidden: UsageKey = { project: { id: ID, kind: 'project', name: null, redacted: true } };
    expect(keyLabel('project', hidden, t)).toBe('A project you cannot see');
    expect(filterValue('project', hidden)).toBeNull();
    expect(filterValue('project', { project: { id: ID, kind: 'project', name: 'Web', redacted: false } })).toBe(ID);
  });

  it('narrows the screen by an id, a routing name or a kind, and the key of every member beyond the largest is none of them', () => {
    expect(filterValue('person', person)).toBe(ID);
    expect(filterValue('model', { model: 'gpt-5.4' })).toBe('gpt-5.4');
    expect(filterValue('agent', { agent: { kind: 'codex', name: 'Codex' } })).toBe('codex');
    expect(isRest({})).toBe(true);
    expect(keyId('model', {})).toBe('rest');
    expect(filterValue('model', {})).toBeNull();
  });
});

/** An answer by the day for three days, split by model: two models and the rest. */
function answer(): UsageAnalytics {
  const day = (n: number) => `2026-10-0${n}T00:00:00.000Z`;
  const split = (a: number, b: number, rest: number) => [
    { key: { model: 'gpt-5.4', model_display: 'GPT-5.4' }, figures: figures(a, a * 1_000, String(a * 10)) },
    { key: { model: 'claude-opus-5', model_display: 'Claude Opus 5' }, figures: figures(b, b * 1_000, String(b * 10)) },
    { key: {}, figures: figures(rest, rest * 1_000, String(rest * 10)) },
  ];
  return {
    scope: 'me',
    from: '2026-10-01',
    to: '2026-10-03',
    bucket: 'day',
    depth: 'own',
    totals: figures(18, 18_000, '180'),
    me: null,
    breakdowns: [],
    series: [
      { at: day(1), figures: figures(0, 0, '0'), by: split(0, 0, 0) },
      { at: day(2), figures: figures(9, 9_000, '90'), by: split(5, 3, 1) },
      { at: day(3), figures: figures(9, 9_000, '90'), by: split(2, 6, 1) },
    ],
  };
}

describe('what the chart is drawn from', () => {
  it('stacks a member for each key, the largest first and the rest last, and the bars add up to the totals', () => {
    const bars = barsOf(answer(), 'turns', 'model', t);
    expect(bars.stacks.map((stack) => stack.label)).toEqual(['Claude Opus 5', 'GPT-5.4', 'Everything else']);
    expect(bars.stacks.map((stack) => stack.total)).toEqual([9, 7, 2]);
    expect(bars.stacks.at(-1)?.rest).toBe(true);
    expect(bars.heights).toEqual([0, 9, 9]);
    expect(bars.stacks.reduce((sum, stack) => sum + stack.total, 0)).toBe(18);
  });

  it('measures what is asked: credits, tokens or turns', () => {
    expect(barsOf(answer(), 'credits', 'model', t).heights).toEqual([0, 90, 90]);
    expect(barsOf(answer(), 'tokens', 'model', t).heights).toEqual([0, 9_000, 9_000]);
  });

  it('is one stack of the totals without a split', () => {
    const bars = barsOf({ ...answer(), series: answer().series.map((point) => ({ ...point, by: [] })) }, 'credits', 'none', t);
    expect(bars.stacks).toHaveLength(1);
    expect(bars.stacks[0]?.label).toBe('Credits');
    expect(bars.heights).toEqual([0, 90, 90]);
  });

  it('leaves out the stretch before the first use for all time, and only then', () => {
    expect(barsOf(answer(), 'credits', 'model', t, true).xs).toHaveLength(2);
    expect(barsOf(answer(), 'credits', 'model', t, false).xs).toHaveLength(3);
  });

  it('knows how long a bucket lasts, a month by the days it has', () => {
    const bars = barsOf({ ...answer(), bucket: 'month', series: [{ at: '2026-02-01T00:00:00.000Z', figures: figures(1, 1, '1'), by: [] }] }, 'turns', 'none', t);
    expect(bars.widths).toEqual([28 * 86_400_000]);
  });

  it('marks a stack made only of use that had no rate, so that it reads "–" in credits and not 0, and no other', () => {
    const day = '2026-10-02T00:00:00.000Z';
    const unrated = (tokens: number): UsageFigures => figures(1, tokens, '0', { unrated_tokens: tokens });
    const bars = barsOf(
      {
        ...answer(),
        series: [
          {
            at: day,
            figures: figures(2, 3_000, '30', { unrated_tokens: 1_000 }),
            by: [
              { key: { model: 'gpt-5.4', model_display: 'GPT-5.4' }, figures: figures(1, 2_000, '30') },
              { key: { model: 'our-local-model', model_display: 'our-local-model' }, figures: unrated(1_000) },
            ],
          },
        ],
      },
      'credits',
      'model',
      t,
    );
    const local = bars.stacks.find((stack) => stack.label === 'our-local-model');
    const rated = bars.stacks.find((stack) => stack.label === 'GPT-5.4');
    expect(local?.total).toBe(0);
    expect(local?.allUnrated).toBe(true);
    expect(local?.unrated).toEqual([true]);
    expect(rated?.allUnrated).toBe(false);
    expect(rated?.unrated).toEqual([false]);
    // A stack that is nothing at all because nothing was used is no stack of use without a rate.
    const quiet = barsOf(answer(), 'credits', 'model', t);
    expect(quiet.stacks.every((stack) => !stack.allUnrated)).toBe(true);
  });

  it('lets a member keep its colour while the chart is split the same way, and the rest of the members stay quiet', () => {
    const first = assignSlots(new Map(), [{ id: 'a', rest: false }, { id: 'b', rest: false }, { id: 'rest', rest: true }]);
    expect([first.get('a'), first.get('b'), first.get('rest')]).toEqual([1, 2, REST_SLOT]);
    // Another span: b drops out, c comes in: a keeps its colour and c takes the free one.
    const second = assignSlots(first, [{ id: 'c', rest: false }, { id: 'a', rest: false }]);
    expect([second.get('a'), second.get('c')]).toEqual([1, 2]);
  });
});

/** A cap as the API gives it, with what a test changes. */
function cap(over: Partial<UsageCap> = {}): UsageCap {
  return {
    id: ID,
    node: { id: ID, kind: 'project', name: 'Web', redacted: false },
    provider: null,
    subject: { kind: 'everyone' },
    split: 'shared',
    models: [],
    tiers: [],
    harness_kinds: [],
    harnesses: [],
    meter: 'tokens',
    period: 'day',
    counted_from: null,
    amount: '2000000',
    at_limit: 'stop',
    warn_percent: 80,
    uncounted: 'allow',
    note: '',
    enabled: true,
    revision: 3,
    created_by: { id: ID, kind: 'human', username: 'dana', display_name: 'Dana', avatar: { text: 'D', color: 1 } },
    created_at: '2026-10-01T09:00:00.000Z',
    updated_at: '2026-10-01T09:00:00.000Z',
    can_edit: true,
    ...over,
  };
}

describe('a day written for a reader', () => {
  it('is written in the language of the page, from a day or from a moment', () => {
    expect(dayWords('2026-10-01', 'en-US')).toBe('Oct 1, 2026');
    expect(dayWords('2026-10-01T09:00:00.000Z', 'en-US')).toBe('Oct 1, 2026');
  });

  it('is written as it came where it is no day, and nothing where there is none, and never makes the page fall', () => {
    expect(dayWords('', 'en-US')).toBe('');
    expect(dayWords(null, 'en-US')).toBe('');
    expect(dayWords(undefined, 'en-US')).toBe('');
    expect(dayWords('sometime', 'en-US')).toBe('sometime');
  });
});

describe('a cap in words', () => {
  it('says the same of a cap in its row and in its sentence', () => {
    const web = cap();
    expect(capTitle(web, t)).toBe('Everyone together on all models in project Web');
    expect(capSentence(web, t, 'en')).toBe('Everyone together may use at most 2M tokens a day on all models in project Web.');
    expect(allowanceWords(web, t, 'en')).toBe('at most 2M tokens a day');
  });

  it('names whom a cap covers in each of the ways', () => {
    expect(capTitle(cap({ split: 'each' }), t)).toContain('Each person');
    expect(capTitle(cap({ subject: { kind: 'role', role: 'developer' }, split: 'each' }), t)).toContain('Each Developer');
    expect(capTitle(cap({ subject: { kind: 'group', group: { id: ID, name: 'Operators', place: 'org North', description: '', member_count: 2, org_id: ID } } }), t)).toContain('Operators, org North together');
    expect(capTitle(cap({ subject: { kind: 'group', group: { id: ID, name: 'Operators', place: 'org North', description: '', member_count: 2, org_id: ID } }, split: 'each' }), t)).toContain('Each member of Operators, org North');
    expect(capTitle(cap({ subject: { kind: 'group', group: { id: ID, name: 'Operators', place: null, description: '', member_count: 2, org_id: ID } } }), t)).toContain('Operators, a place you cannot see together');
    const person=cap({ subject: { kind: 'user', user: { id: ID, kind: 'human', username: 'bo', display_name: 'Bo', address:'acme.bo', avatar: { text: 'B', color: 1 } } } });
    expect(capTitle(person,t)).toContain('Bo on');expect(capTitle(person,t)).not.toContain('acme.bo');
  });

  it('names models, a family by its beginning, a group of a provider\'s models, and the speed Fast', () => {
    expect(capSentence(cap({ models: ['gpt-5.4', 'gpt-6*', '@frontier'] }), t, 'en')).toContain('on gpt-5.4, gpt-6…, the frontier group of models');
    const declared = { kind: 'codex', name: 'Codex', names: {} };
    expect(capSentence(cap({ tiers: ['fast'], harness_kinds: ['codex'] }), t, 'en', () => programName(declared, 'en'))).toContain('all models with Codex at the speed Fast');
  });

  it('uses a third program declaration in either language and never derives a name from its kind', async () => {
    const declared = { kind: 'private-research', name: 'Research assistant', names: { 'zh-CN': '研究助理' } };
    const rule = cap({ harness_kinds: [declared.kind] });
    expect(capTitle(rule, t, () => programName(declared, 'en'))).toContain('with Research assistant');
    const zh = await import('@/i18n/zh-CN.json');
    i18n.addResourceBundle('zh-CN', 'translation', zh.default);
    const chinese = i18n.getFixedT('zh-CN');
    expect(capTitle(rule, chinese, () => programName(declared, 'zh-CN'))).toContain('研究助理');
    expect(capTitle(rule, t)).toContain('1 agent program (name not reported)');
    expect(capTitle(rule, t)).not.toContain(declared.kind);
  });

  it('formats one page of a hundred thousand selected programs without copying or reading the remainder', () => {
    const kinds = Array.from({ length: 100_000 }, (_, index) => `program-${index}`);
    const name = vi.fn((kind: string) => `Declared ${kind}`);
    const sentence = capSentence(cap({ harness_kinds: kinds }), t, 'en', name);
    expect(name).toHaveBeenCalledTimes(DETAIL_PAGE_ROWS);
    expect(sentence).toContain('Declared program-0');
    expect(sentence).toContain(`Declared program-${DETAIL_PAGE_ROWS - 1}`);
    expect(sentence).not.toContain(`Declared program-${DETAIL_PAGE_ROWS},`);
    expect(sentence).toContain('99,988 more');
    expect(sentence.length).toBeLessThan(1000);
  });

  it.each(['en', 'zh-CN'])('reads only twelve supplied names out of a hundred thousand selected programs in %s', (locale) => {
    let reads = 0;
    const harnesses = Array.from({ length: 100_000 }, (_, index) => ({
      kind: `program-${index}`,
      get name() {
        if (index >= DETAIL_PAGE_ROWS) throw new Error('A cap preview must not read the remaining names');
        reads += 1; return `Declared program ${index}`;
      },
      names: { 'zh-CN': `声明程序 ${index}` },
    }));
    const sentence = capSentence(cap({ harness_kinds: harnesses.map((program) => program.kind), harnesses }), t, locale);
    expect(sentence).toContain(locale === 'en' ? 'Declared program 0' : '声明程序 0');
    expect(sentence).toContain(locale === 'en' ? `Declared program ${DETAIL_PAGE_ROWS - 1}` : `声明程序 ${DETAIL_PAGE_ROWS - 1}`);
    expect(sentence).not.toContain(locale === 'en' ? `Declared program ${DETAIL_PAGE_ROWS},` : `声明程序 ${DETAIL_PAGE_ROWS},`);
    expect(sentence).toContain('99,988 more');
    expect(sentence.length).toBeLessThan(1000);
    expect(reads).toBe(locale === 'en' ? DETAIL_PAGE_ROWS : 0);
  });

  it('puts a cap on a provider "on" it, and a cap on turns at once without a period', () => {
    const provider = cap({ node: null, provider: { id: ID, name: 'Team Pro', kind: 'subscription', harness_kind: null, harness: null, address: '', placement: 'managed', status: 'ready' }, meter: 'running', period: null, amount: '4' });
    expect(capSentence(provider, t, 'en')).toBe('Everyone together may run at most 4 turns at once on all models on Team Pro.');
    expect(capSentence(cap({ node: null }), t, 'en')).toContain('everywhere');
  });

  it('says what is used of how much, and when a cap starts again in UTC', () => {
    const status = { cap: cap(), used: '2140000' } as Pick<CapStatus, 'used'> & { cap: UsageCap };
    expect(usedWords(status, t, 'en')).toBe('2.1M of 2M tokens');
    expect(startsAgain('2026-10-13T00:00:00.000Z', 'day', 'en', t)).toBe('starts again 00:00 UTC');
    expect(startsAgain('2026-10-19T00:00:00.000Z', 'week', 'en', t)).toBe('starts again Monday');
    expect(startsAgain('2026-11-01T00:00:00.000Z', 'month', 'en', t)).toBe('starts again November 1');
  });

  it('is told in Chinese as well', async () => {
    const zh = await import('@/i18n/zh-CN.json');
    i18n.addResourceBundle('zh-CN', 'translation', zh.default);
    const chinese = i18n.getFixedT('zh-CN');
    expect(capSentence(cap(), chinese, 'zh-CN')).toBe('所有人合计在项目 Web 中，每天在所有模型上最多可使用 2M 令牌。');
  });
});

describe('a cap that is being made', () => {
  const t_ = t;

  it('is counted for each person when it names a role or each member of a group, and for one person it is theirs alone', () => {
    expect(['everyone', 'each', 'user', 'group', 'eachGroup', 'role'].map((who) => splitOf(who as never))).toEqual(['shared', 'each', 'shared', 'shared', 'each', 'each']);
    expect(subjectOf(emptyDraft({ who: 'role', role: 'admin' }))).toEqual({ kind: 'role', role: 'admin' });
    expect(subjectOf(emptyDraft({ who: 'eachGroup', groupId: ID }))).toEqual({ kind: 'group', id: ID });
  });

  it('takes an amount of tokens with k and M, whole tokens and turns, and credits to six decimals', () => {
    expect(amountText('2M', 'tokens')).toBe('2000000');
    expect(amountText('1.5m', 'tokens')).toBe('1500000');
    expect(amountText('500k', 'tokens')).toBe('500000');
    expect(amountText('2,000,000', 'tokens')).toBe('2000000');
    expect(amountText('0.5', 'tokens')).toBeNull();
    expect(amountText('2M', 'running')).toBeNull();
    expect(amountText('4', 'running')).toBe('4');
    expect(amountText('2.50', 'credits')).toBe('2.5');
    expect(amountText('0.1234567', 'credits')).toBeNull();
    expect(amountText('', 'tokens')).toBeNull();
  });

  it('sends nothing of a period, a warning or an answer to uncounted work for a cap on turns at once', () => {
    const body = inputOf(emptyDraft({ meter: 'running', amount: '4', who: 'each' }), { providerId: ID });
    expect(body).toEqual({ provider_id: ID, subject: { kind: 'everyone' }, split: 'each', models: [], tiers: [], harness_kinds: [], meter: 'running', amount: '4' });
  });

  it('sends a day\'s tokens with the warning, what happens at the cap and the models, families, groups and speeds it names', () => {
    const body = inputOf(emptyDraft({ amount: '2M', models: ['gpt-6*', '@frontier'], tiers: ['fast'], harnessKinds: ['codex'], note: 'for Web' }), { nodeId: ID });
    expect(body).toEqual({
      node_id: ID,
      subject: { kind: 'everyone' },
      split: 'shared',
      models: ['gpt-6*', '@frontier'],
      tiers: ['fast'],
      harness_kinds: ['codex'],
      meter: 'tokens',
      amount: '2000000',
      period: 'day',
      at_limit: 'stop',
      uncounted: 'allow',
      warn_percent: 80,
      note: 'for Web',
    });
  });

  it('exempts with no amount at all', () => {
    expect(inputOf(emptyDraft({ noLimit: true, who: 'user', userId: ID }), { nodeId: ID }).amount).toBeNull();
  });

  it('writes a family with an ellipsis as a pattern, in lower case', () => {
    expect(modelPattern('GPT-6.1…')).toBe('gpt-6.1*');
    expect(modelPattern(' @Frontier ')).toBe('@frontier');
  });

  it('says what is wrong before it is sent, by what it is about', () => {
    expect(check(emptyDraft(), t_)).toMatchObject({ amount: 'Write an amount, or tick No limit.' });
    expect(check(emptyDraft({ amount: '0.5' }), t_).amount).toContain('whole number');
    expect(check(emptyDraft({ amount: '5', who: 'user' }), t_).subject).toBe('Choose a person.');
    expect(check(emptyDraft({ amount: '5', who: 'group' }), t_).subject).toBe('Choose a group.');
    expect(check(emptyDraft({ amount: '5', warn: '100' }), t_).warn_percent).toContain('1 to 99');
    expect(check(emptyDraft({ amount: '5', models: ['@Bad Group'] }), t_).models).toBeDefined();
    expect(check(emptyDraft({ amount: '5', period: 'total', countedFrom: '2026-02-31' }), t_).counted_from).toBeDefined();
    expect(check(emptyDraft({ noLimit: true }), t_)).toEqual({});
    expect(check(emptyDraft({ meter: 'running', amount: '3', warn: '500' }), t_)).toEqual({});
  });

  it('names only what changed in a cap that exists, against the revision that was read', () => {
    const web = cap();
    const same = updateOf(draftOf(web), web);
    expect(same).toEqual({ expected_revision: 3 });
    expect(isChange(same)).toBe(false);
    const raised = updateOf({ ...draftOf(web), amount: '3M', models: ['@frontier'], warn: '' }, web);
    expect(raised).toEqual({ expected_revision: 3, amount: '3000000', models: ['@frontier'], warn_percent: null });
    expect(isChange(raised)).toBe(true);
  });
});

describe('the tightest cap of a thread', () => {
  const status = (amount: string | null, used: string, running = '0', over: Partial<UsageCap> = {}) => ({ cap: cap({ amount, ...over }), used, in_progress: running }) as CapStatus;

  it('counts what turns still running have reported, and leaves out caps on turns at once, switched-off caps and exemptions', () => {
    expect(tightestPercent([status('1000', '400', '240'), status('1000', '200')])).toBe(64);
    expect(tightestPercent([status('4', '3', '0', { meter: 'running', period: null })])).toBeNull();
    expect(tightestPercent([status('1000', '900', '0', { enabled: false })])).toBeNull();
    expect(tightestPercent([status(null, '900')])).toBeNull();
    expect(tightestPercent([status('1000', '1500')])).toBe(150);
  });
});
