// @vitest-environment node
/** How the finder ranks what it holds itself (settings, commands, rows of a page, help) and what it makes of the sign at the start of the field. */
import { describe, expect, it } from 'vitest';

import { answer, entriesOf } from '@/features/finder/local';
import { isBare, parseField, withMode } from '@/features/finder/modes';
import { fold, markedRanges, matches, matchScore, prepare, rank, wordsOf } from '@/features/finder/ranking';

const entries = (titles: readonly (string | { title: string; words?: string[]; detail?: string })[]) =>
  entriesOf(titles, (item) => (typeof item === 'string' ? { title: item } : item));
const titlesOf = (found: { item: string | { title: string } }[]) => found.map(({ item }) => (typeof item === 'string' ? item : item.title));

describe('ranking what the finder holds', () => {
  const settings = entries([
    { title: 'Stop when idle', words: ['env.idle_stop_minutes'], detail: 'Stop a managed environment after this many minutes without activity.' },
    { title: 'Sandbox', words: ['sandbox.mode'], detail: 'What the harness’s own sandbox lets an agent’s commands do.' },
    { title: 'Approval mode', words: ['approval.mode'], detail: 'When an agent has to ask before it acts.' },
    { title: 'Idle timeout of shells', words: ['terminal.idle_minutes'], detail: 'How long an unused shell stays open.' },
  ]);

  it('finds a setting by what it does, whatever the order of the words', () => {
    expect(titlesOf(answer(settings, 'idle stop'))[0]).toBe('Stop when idle');
    expect(titlesOf(answer(settings, 'stop idle'))[0]).toBe('Stop when idle');
  });

  it('needs every word typed to be found somewhere in an entry', () => {
    expect(answer(settings, 'idle zebra')).toEqual([]);
  });

  it('puts a title that begins with the word before one that has it later, and both before one that only explains', () => {
    const found = titlesOf(answer(settings, 'idle'));
    expect(found).toEqual(['Idle timeout of shells', 'Stop when idle']);
    // "unused" is in the sentence of one entry only: a sentence finds, but only after what is named so.
    expect(titlesOf(answer(settings, 'sandbox'))).toEqual(['Sandbox']);
    expect(titlesOf(answer(settings, 'activity'))).toEqual(['Stop when idle']);
  });

  it('finds by the key a person who knows it types, and by its parts', () => {
    expect(titlesOf(answer(settings, 'env.idle_stop_minutes'))).toEqual(['Stop when idle']);
    expect(titlesOf(answer(settings, 'sandbox mode'))[0]).toBe('Sandbox');
  });

  it('counts a title above the words an entry is found by, and those above its sentence', () => {
    const mixed = entries([
      { title: 'Alpha', detail: 'beta' },
      { title: 'Gamma', words: ['beta'] },
      { title: 'Beta' },
    ]);
    expect(titlesOf(answer(mixed, 'beta'))).toEqual(['Beta', 'Gamma', 'Alpha']);
  });

  it('is no matter of case or accents', () => {
    const accented = entries(['Résumé du fil', 'Other']);
    expect(titlesOf(answer(accented, 'RESUME'))).toEqual(['Résumé du fil']);
    expect(fold('Straße')).toHaveLength('Straße'.length);
  });

  it('finds inside a word, which is the only way in a script that has no spaces', () => {
    const chinese = entries(['空闲时停止', '沙箱模式', '审批模式']);
    expect(titlesOf(answer(chinese, '模式'))).toEqual(['沙箱模式', '审批模式']);
    expect(titlesOf(answer(chinese, '停止'))).toEqual(['空闲时停止']);
  });

  it('requires all words, including the words of a question', () => {
    const rows = entries([{ title: 'Archive Topic', detail: 'Archive a topic and bring it back.' }, { title: 'View Thread' }]);
    expect(titlesOf(answer(rows, 'who can archive'))).toEqual([]);
    expect(wordsOf('who can')).toEqual(['who', 'can']);
  });

  it('uses unordered words, exact quoted phrases and the same name tiers in every local filter', () => {
    expect(wordsOf('redirect "fix login"')).toEqual(['redirect', 'fix login']);
    expect(matches('login fix', 'Fix login redirect')).toBe(true);
    expect(matches('"login fix"', 'Fix login redirect')).toBe(false);
    expect(matches('redirect "fix login"', 'Fix login redirect')).toBe(true);
    expect(matchScore('login', 'login')).toBeGreaterThan(matchScore('login redirect', 'login'));
    expect(matchScore('login redirect', 'login')).toBeGreaterThan(matchScore('Fix login', 'login'));
    expect(matchScore('Fix login', 'login')).toBeGreaterThan(matchScore('prelogin', 'login'));
  });

  it('keeps the order entries were given in when they answer alike, so that the list does not shuffle with each letter', () => {
    const alike = entries(['Open a', 'Open b', 'Open c']);
    expect(titlesOf(answer(alike, 'open'))).toEqual(['Open a', 'Open b', 'Open c']);
    expect(titlesOf(answer(alike, 'open '))).toEqual(['Open a', 'Open b', 'Open c']);
  });

  it('lists everything in its own order when nothing is typed', () => {
    expect(titlesOf(answer(settings, ''))).toHaveLength(4);
    expect(titlesOf(answer(settings, '   '))[0]).toBe('Stop when idle');
  });

  it('answers within its budget over a thousand entries', () => {
    const many = entriesOf(
      Array.from({ length: 1_000 }, (_, index) => ({ title: `Setting number ${index}`, words: [`area.key_${index}`], detail: `Explains what setting ${index} does to the work of an agent.` })),
      (item) => item,
    );
    const start = performance.now();
    for (let key = 0; key < 20; key += 1) answer(many, 'setting number 99');
    // Each key is ranked in a few milliseconds; the budget is fifty.
    expect((performance.now() - start) / 20).toBeLessThan(50);
  });
});

describe('what is marked in a name', () => {
  it('marks every word typed where it stands, and merges what touches', () => {
    expect(markedRanges('Fix login redirect', 'fix red')).toEqual([
      [0, 3],
      [10, 13],
    ]);
    expect(markedRanges('abcd', 'ab bc')).toEqual([[0, 3]]);
  });

  it('lines up with the original text although the match ignores case and accents', () => {
    const text = 'Été à Paris';
    const range = markedRanges(text, 'ete')[0];
    expect(text.slice(range?.[0], range?.[1])).toBe('Été');
  });

  it('marks nothing for words that are not there', () => {
    expect(markedRanges('Fix login', 'zzz')).toEqual([]);
  });
});

describe('preparing an entry', () => {
  it('folds the title, the words and the sentence once', () => {
    expect(prepare({ title: 'Éa', words: ['Ü'], detail: 'X' })).toEqual({ title: 'ea', words: 'u', detail: 'x' });
  });

  it('ranks by a function of the entry, with a limit', () => {
    const ranked = rank(['b', 'a', 'ab'], (entry) => prepare({ title: entry }), 'a', 1);
    expect(ranked.map((one) => one.entry)).toEqual(['a']);
  });
});

describe('the sign at the start of the field', () => {
  it('chooses a mode by its first character and leaves the words after it', () => {
    expect(parseField('> new topic')).toEqual({ mode: 'commands', query: 'new topic' });
    expect(parseField('@dora')).toEqual({ mode: 'people', query: 'dora' });
    expect(parseField(': idle stop')).toEqual({ mode: 'settings', query: 'idle stop' });
    expect(parseField('? fork')).toEqual({ mode: 'help', query: 'fork' });
    expect(parseField('/redirect loop')).toEqual({ mode: 'conversations', query: 'redirect loop' });
  });

  it('looks everywhere with no sign, and a sign later in the text is only a letter', () => {
    expect(parseField('fix > login')).toEqual({ mode: 'all', query: 'fix > login' });
    expect(parseField('  fix ')).toEqual({ mode: 'all', query: 'fix' });
  });

  it('says when only a sign has been typed', () => {
    expect(isBare('>')).toBe(true);
    expect(isBare('> ')).toBe(true);
    expect(isBare('> x')).toBe(false);
    expect(isBare('')).toBe(false);
  });

  it('is written and taken away by the buttons under the field, keeping the words', () => {
    expect(withMode('fix', 'commands')).toBe('> fix');
    expect(withMode('> fix', 'people')).toBe('@ fix');
    expect(withMode('@ fix', 'all')).toBe('fix');
    expect(withMode('', 'help')).toBe('?');
  });
});
