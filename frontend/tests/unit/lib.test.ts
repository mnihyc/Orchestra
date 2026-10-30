/** Times, numbers, sizes, the one character of an avatar, and which keys are shortcuts: as each language and each keyboard needs them. */
import { describe, expect, it } from 'vitest';

import { formatBytes, formatNumber, formatPercent, formatTokens } from '@/lib/format';
import { avatarText, graphemeCount, graphemes, IDENTITY_COLORS, identityColor } from '@/lib/grapheme';
import { retryPause, retryRead, NetworkError, newActKey } from '@/lib/requests';
import { matchesChord, parseShortcut, ShortcutBook, shortcutCaps, takesTyping } from '@/lib/shortcuts';
import { calendarDay, formatCompactTime, formatDateTime, formatRelative, formatSpan, formatTimeOfDay } from '@/lib/time';

const NOW = Date.parse('2026-10-11T12:00:00Z');
const en = { locale: 'en', timeZone: 'UTC' };
const zh = { locale: 'zh-CN', timeZone: 'Asia/Shanghai' };

describe('a time', () => {
  it('keeps a row time whole with the shortest form for its local calendar group', () => {
    expect(formatCompactTime(NOW - 10_000, en, NOW)).toBe('now');
    expect(formatCompactTime(NOW - 5 * 60_000, en, NOW)).toBe('5 min');
    expect(formatCompactTime('2026-10-11T09:30:00Z', en, NOW)).toBe('09:30');
    expect(formatCompactTime('2026-10-10T09:30:00Z', en, NOW)).toBe('09:30');
    expect(formatCompactTime('2026-10-08T09:30:00Z', en, NOW)).toBe('Thu');
    expect(formatCompactTime('2026-10-01T09:30:00Z', en, NOW)).toBe('Oct 1');
    expect(formatCompactTime('2025-10-01T09:30:00Z', en, NOW)).toBe('Oct 1, 2025');
    expect(formatCompactTime(NOW - 5 * 60_000, zh, NOW)).toBe('5分钟');
    expect(formatCompactTime('2026-10-10T09:30:00Z', zh, NOW)).toBe('17:30');
    const zone = { locale: 'en', timeZone: 'America/New_York' };
    expect(calendarDay('2026-11-02T05:30:00Z', zone) - calendarDay('2026-11-01T04:30:00Z', zone)).toBe(1);
    expect(formatCompactTime('2026-11-01T04:30:00Z', zone, '2026-11-02T05:30:00Z')).toBe('00:30');
  });
  it('is said relative to now while it is under a day old', () => {
    expect(formatRelative(NOW - 10_000, en, NOW)).toBe('now');
    expect(formatRelative(NOW - 3 * 60_000, en, NOW)).toBe('3 min. ago');
    expect(formatRelative(NOW - 5 * 3_600_000, en, NOW)).toBe('5 hr. ago');
    expect(formatRelative(NOW - 3 * 60_000, zh, NOW)).toBe('3分钟前');
    expect(formatRelative(NOW + 2 * 3_600_000, zh, NOW)).toBe('2小时后');
  });

  it('is a date and a time of day after that, in the reader\'s own zone, with the year only when it is another one', () => {
    expect(formatRelative(NOW - 3 * 86_400_000, en, NOW)).toBe('Oct 8, 12:00 PM');
    expect(formatDateTime('2025-03-01T08:30:00Z', en, NOW)).toBe('Mar 1, 2025, 08:30 AM');
    // The same moment is the evening of that day in Shanghai.
    expect(formatDateTime(NOW - 3 * 86_400_000, zh, NOW)).toBe('10月8日 20:00');
  });

  it('is the time of day alone when it is today, as a refusal says until when', () => {
    expect(formatTimeOfDay(NOW + 2 * 3_600_000 + 5 * 60_000, en, NOW)).toBe('02:05 PM');
    expect(formatTimeOfDay(NOW + 2 * 3_600_000 + 5 * 60_000, zh, NOW)).toBe('22:05');
    // Tomorrow in the reader's zone is said with its date.
    expect(formatTimeOfDay(NOW + 20 * 3_600_000, en, NOW)).toBe('Oct 12, 08:00 AM');
  });

  it('names a span by its two largest units', () => {
    expect(formatSpan(45_000, en)).toBe('45s');
    expect(formatSpan(125 * 60_000, en)).toBe('2h 5m');
    expect(formatSpan(0, en)).toBe('0s');
    expect(formatSpan(26 * 3_600_000, zh)).toBe('1天 2小时');
  });

  it('uses full unit words where a labelled grant says how long it lasts', () => {
    expect(formatSpan(7 * 86_400_000, en, 'long')).toBe('7 days');
    expect(formatSpan(7 * 86_400_000, zh, 'long')).toBe('7天');
    expect(formatSpan(26 * 3_600_000, en, 'long')).toBe('1 day 2 hours');
    expect(formatSpan(7 * 86_400_000, en)).toBe('7d');
  });
});

describe('a number', () => {
  it('is grouped as its language groups it', () => {
    expect(formatNumber(1234567.5, 'en')).toBe('1,234,567.5');
    expect(formatNumber(1234567.5, 'zh-CN')).toBe('1,234,567.5');
    expect(formatPercent(0.64, 'en')).toBe('64%');
  });

  it('of bytes is said in binary units', () => {
    expect(formatBytes(0, 'en')).toBe('0 B');
    expect(formatBytes(1023, 'en')).toBe('1,023 B');
    expect(formatBytes(1024, 'en')).toBe('1 KiB');
    expect(formatBytes(1536, 'en')).toBe('1.5 KiB');
    expect(formatBytes(20 * 1024 * 1024, 'zh-CN')).toBe('20 MiB');
    expect(formatBytes(5 * 1024 ** 3 + 1024 ** 3 / 4, 'en')).toBe('5.3 GiB');
  });

  it('of tokens is written out up to ten thousand and abbreviated above', () => {
    expect(formatTokens(9_999, 'en')).toBe('9,999');
    expect(formatTokens(10_000, 'en')).toBe('10,000');
    expect(formatTokens(12_400, 'en')).toBe('12.4k');
    expect(formatTokens(2_000_000, 'en')).toBe('2M');
  });
});

describe('the one character of an avatar', () => {
  it('is the first one of a name as a reader sees it', () => {
    expect(avatarText('ada')).toBe('A');
    expect(avatarText('  grace hopper')).toBe('G');
    expect(avatarText('王小明')).toBe('王');
    expect(avatarText('émile')).toBe('É');
    // A letter written with a combining accent is one character, and so is a family of four.
    expect(avatarText('émile')).toBe('É');
    expect(avatarText('👨‍👩‍👧‍👦 family')).toBe('👨‍👩‍👧‍👦');
    expect(avatarText('🇨🇭 team')).toBe('🇨🇭');
    expect(avatarText('   ')).toBe('');
  });

  it('counts a text by what a reader sees, not by how it is stored', () => {
    expect(graphemeCount('é')).toBe(1);
    expect(graphemes('a👍🏽王')).toEqual(['a', '👍🏽', '王']);
  });

  it('always has the same colour for the same thing, and one of the twelve', () => {
    const colour = identityColor('00000000-0000-4000-8002-000000000064');
    expect(identityColor('00000000-0000-4000-8002-000000000064')).toBe(colour);
    expect(colour).toBeGreaterThanOrEqual(0);
    expect(colour).toBeLessThan(IDENTITY_COLORS);
    const spread = new Set(Array.from({ length: 200 }, (_, index) => identityColor(`thing-${index}`)));
    expect(spread.size).toBe(IDENTITY_COLORS);
  });
});

const key = (init: KeyboardEventInit & { consumedByInputMethod?: boolean }, target?: HTMLElement): KeyboardEvent => {
  const event = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });
  // What a browser reports for a key the input method took before it says that it is composing.
  if (init.consumedByInputMethod === true) Object.defineProperty(event, 'keyCode', { value: 229 });
  if (target !== undefined) Object.defineProperty(event, 'target', { value: target });
  return event;
};

describe('a shortcut', () => {
  it('is the Command key on Apple\'s systems and Control elsewhere', () => {
    const [chord] = parseShortcut('Mod+K');
    if (chord === undefined) throw new Error('no chord');

    expect(matchesChord(key({ key: 'k', code: 'KeyK', ctrlKey: true }), chord, false)).toBe(true);
    expect(matchesChord(key({ key: 'k', code: 'KeyK', metaKey: true }), chord, false)).toBe(false);
    expect(matchesChord(key({ key: 'k', code: 'KeyK', metaKey: true }), chord, true)).toBe(true);
    expect(shortcutCaps('Mod+K', true)).toEqual([['⌘', 'K']]);
    expect(shortcutCaps('Mod+K', false)).toEqual([['Ctrl', 'K']]);
    expect(shortcutCaps('G I', false)).toEqual([['G'], ['I']]);
    expect(shortcutCaps('Shift+F6', false)).toEqual([['Shift', 'F6']]);
  });

  it('never takes a key from an input method that is composing a character', () => {
    const [chord] = parseShortcut('Mod+K');
    if (chord === undefined) throw new Error('no chord');

    expect(matchesChord(key({ key: 'k', code: 'KeyK', ctrlKey: true, isComposing: true }), chord, false)).toBe(false);
    // The key that the input method consumed, before the browser says that it is composing.
    expect(matchesChord(key({ key: 'Process', code: 'KeyK', ctrlKey: true, consumedByInputMethod: true }), chord, false)).toBe(false);
  });

  it('that types a character is left to a field, and one that types nothing works in it', () => {
    const field = document.createElement('input');
    const terminal = document.createElement('div');
    terminal.dataset.keys = 'own';
    const [question] = parseShortcut('?');
    const [finder] = parseShortcut('Mod+K');
    const [help] = parseShortcut('F1');
    if (question === undefined || finder === undefined || help === undefined) throw new Error('no chord');

    expect(takesTyping(field)).toBe(true);
    expect(takesTyping(terminal)).toBe(true);
    expect(takesTyping(document.createElement('button'))).toBe(false);
    expect(matchesChord(key({ key: '?', shiftKey: true }, field), question, false)).toBe(false);
    expect(matchesChord(key({ key: '?', shiftKey: true }, document.body), question, false)).toBe(true);
    expect(matchesChord(key({ key: 'k', code: 'KeyK', ctrlKey: true }, field), finder, false)).toBe(true);
    expect(matchesChord(key({ key: 'F1', code: 'F1' }, field), help, false)).toBe(true);
  });

  it('is the letter printed on the key, also where the layout types another script', () => {
    const [chord] = parseShortcut('Mod+K');
    if (chord === undefined) throw new Error('no chord');

    // A Russian layout types "л" with the key that carries K.
    expect(matchesChord(key({ key: 'л', code: 'KeyK', ctrlKey: true }), chord, false)).toBe(true);
    // A French layout has A where the key code says Q: the letter printed on the key wins.
    const [a] = parseShortcut('Mod+A');
    if (a === undefined) throw new Error('no chord');
    expect(matchesChord(key({ key: 'a', code: 'KeyQ', ctrlKey: true }), a, false)).toBe(true);
  });

  it('of two keys takes the second within a second of the first, and then no longer', () => {
    let now = 0;
    const book = new ShortcutBook(false, () => now);
    const went: string[] = [];
    book.add('inbox', 'G I', () => went.push('inbox'));
    book.add('approvals', 'G A', () => went.push('approvals'));

    expect(book.handle(key({ key: 'g', code: 'KeyG' }))).toBe(true);
    now = 400;
    expect(book.handle(key({ key: 'a', code: 'KeyA' }))).toBe(true);
    expect(went).toEqual(['approvals']);

    now = 5_000;
    book.handle(key({ key: 'g', code: 'KeyG' }));
    now = 6_500;
    expect(book.handle(key({ key: 'i', code: 'KeyI' }))).toBe(false);
    expect(went).toEqual(['approvals']);
  });

  it('that did nothing leaves the key to the page', () => {
    const book = new ShortcutBook(false);
    book.add('close', 'Alt+W', () => false);
    const event = key({ key: 'w', code: 'KeyW', altKey: true });

    expect(book.handle(event)).toBe(false);
    expect(event.defaultPrevented).toBe(false);
  });
});

describe('asking again', () => {
  it('is for a read that got no answer, three times, after one, two and four seconds and a little more', () => {
    const lost = new NetworkError('failed');
    expect([0, 1, 2, 3].map((failures) => retryRead(failures, lost))).toEqual([true, true, true, false]);
    expect([0, 1, 2].map((attempt) => retryPause(attempt, () => 0))).toEqual([1_000, 2_000, 4_000]);
    expect(retryPause(0, () => 1)).toBe(1_250);
  });

  it('is not for an answer of the API, nor for a request that was called off', () => {
    expect(retryRead(0, new Error('refused'))).toBe(false);
    expect(retryRead(0, new NetworkError('aborted'))).toBe(false);
  });

  it('uses a key that is made once for each act', () => {
    const keys = new Set(Array.from({ length: 50 }, () => newActKey()));
    expect(keys.size).toBe(50);
    expect([...keys][0]).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  });
});
