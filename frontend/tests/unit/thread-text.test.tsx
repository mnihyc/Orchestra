/** The text a thread shows: a difference with its changed words marked, output with its colours, Markdown without markup of its own, and each step in words. */
import { render } from '@testing-library/react';
import type { TFunction } from 'i18next';
import { describe, expect, it } from 'vitest';

import { describeStep, stepFailed, type ToolItem } from '@/features/timeline/titles';
import { i18n } from '@/i18n';
import { parseAnsi, stripAnsi } from '@/lib/ansi';
import { countChanges, type Hunk, parseDiff, sideBySide, tokens, unchangedBetween } from '@/lib/diff';
import { isSafeHref, lexMarkdown, renderBlocks } from '@/lib/markdown';

import { change, command, fileChange, fileRead, otherTool } from '../doubles/thread-rows';

const t = i18n.t.bind(i18n) as TFunction;

const DIFF = [
  '--- a/src/login.ts',
  '+++ b/src/login.ts',
  '@@ -3,5 +3,5 @@ function login() {',
  ' one',
  ' two',
  '-  return redirect(next);',
  '+  return redirect(next ?? "/");',
  ' four',
  ' five',
  '@@ -20,3 +20,4 @@',
  ' alpha',
  '+beta',
  ' gamma',
  ' delta',
  '\\ No newline at end of file',
].join('\n');

/** The two hunks of the difference above. */
function twoHunks(): [Hunk, Hunk] {
  const [first, second] = parseDiff(DIFF);
  if (first === undefined || second === undefined) throw new Error('the difference has two hunks');
  return [first, second];
}

describe('a difference', () => {
  it('is read into hunks, with the numbers of both sides on every line and the file headers left out', () => {
    const hunks = parseDiff(DIFF);
    expect(hunks).toHaveLength(2);
    const first = hunks[0];
    expect(first?.heading).toBe('function login() {');
    expect(first?.lines.map((line) => [line.kind, line.oldNo, line.newNo])).toEqual([
      ['context', 3, 3],
      ['context', 4, 4],
      ['del', 5, undefined],
      ['add', undefined, 5],
      ['context', 6, 6],
      ['context', 7, 7],
    ]);
    // A note that the last line has no newline belongs to the line before it.
    expect(hunks[1]?.lines.at(-1)?.noNewline).toBe(true);
  });

  it('marks the words that changed inside a changed line, and says nothing of the words that stayed', () => {
    const [first] = twoHunks();
    const removed = first.lines.find((line) => line.kind === 'del');
    const added = first.lines.find((line) => line.kind === 'add');
    expect(removed?.marks).toEqual([]);
    expect(added?.marks).toHaveLength(1);
    const [from, to] = added?.marks?.[0] ?? [0, 0];
    expect(added?.text.slice(from, to)).toBe('?? "/"');
  });

  it('marks nothing in a line that shares almost nothing with the line it replaced', () => {
    const [hunk] = parseDiff(['@@ -1 +1 @@', '-alpha beta gamma delta epsilon zeta eta theta', '+completely different words here now'].join('\n'));
    expect(hunk?.lines.every((line) => line.marks === undefined)).toBe(true);
  });

  it('is laid side by side with a removed line facing the line that took its place', () => {
    const [first, second] = twoHunks();
    const rows = sideBySide(first);
    expect(rows).toHaveLength(5);
    const changed = rows[2];
    expect(changed?.left?.kind).toBe('del');
    expect(changed?.right?.kind).toBe('add');
    // An added line with nothing before it has an empty side.
    const added = sideBySide(second).find((row) => row.left === null);
    expect(added?.right?.text).toBe('beta');
  });

  it('counts the lines it adds and removes, and the lines that lie between hunks', () => {
    const [first, second] = twoHunks();
    expect(countChanges([first, second])).toEqual({ added: 2, removed: 1 });
    expect(unchangedBetween(first, second)).toBe(12);
  });

  it('splits a line into words, runs of spaces and single marks', () => {
    expect(tokens('a  b=1')).toEqual(['a', '  ', 'b', '=', '1']);
    expect(tokens('日本語 text')).toEqual(['日本語', ' ', 'text']);
  });
});

describe('output with escapes', () => {
  it('keeps the colour of a run and drops every other escape', () => {
    const spans = parseAnsi('\u001b[31mred\u001b[0m plain \u001b[2Jcleared\u001b]0;a title\u0007 \u001b[1;32mbold green\u001b[0m');
    expect(spans.map((span) => span.text)).toEqual(['red', ' plain cleared ', 'bold green']);
    expect(spans[0]?.style.fg).toEqual({ kind: 'named', index: 1 });
    expect(spans[1]?.style).toEqual({});
    expect(spans[2]?.style).toEqual({ bold: true, fg: { kind: 'named', index: 2 } });
  });

  it('is one span with no style where there is no escape', () => {
    expect(parseAnsi('just text\nsecond line')).toEqual([{ text: 'just text\nsecond line', style: {} }]);
  });

  it('leaves the last version of a line that a carriage return rewrote', () => {
    expect(stripAnsi('10%\r50%\r100%\ndone')).toBe('100%\ndone');
    expect(stripAnsi('line one\r\nline two')).toBe('line one\nline two');
  });

  it('reads 256 colours and true colours', () => {
    const spans = parseAnsi('\u001b[38;5;196mhot\u001b[38;2;1;2;3mrgb');
    expect(spans[0]?.style.fg).toEqual({ kind: 'rgb', r: 255, g: 0, b: 0 });
    expect(spans[1]?.style.fg).toEqual({ kind: 'rgb', r: 1, g: 2, b: 3 });
  });
});

describe('Markdown', () => {
  const draw = (source: string) => render(<div>{renderBlocks(lexMarkdown(source))}</div>).container;

  it('shows raw HTML as the text it is, and brings no element of its own', () => {
    const page = draw('Before <script>alert(1)</script> after\n\n<img src=x onerror=alert(1)>\n\n<div class="x">block</div>');
    expect(page.querySelector('script')).toBeNull();
    expect(page.querySelector('img')).toBeNull();
    expect(page.querySelector('.x')).toBeNull();
    expect(page.textContent).toContain('<script>alert(1)</script>');
    expect(page.textContent).toContain('<img src=x onerror=alert(1)>');
  });

  it('follows only addresses that are safe', () => {
    expect(isSafeHref('https://example.org/a')).toBe(true);
    expect(isSafeHref('http://example.org')).toBe(true);
    expect(isSafeHref('mailto:a@example.org')).toBe(true);
    expect(isSafeHref('/thread/abc')).toBe(true);
    expect(isSafeHref('#section')).toBe(true);
    expect(isSafeHref('javascript:alert(1)')).toBe(false);
    expect(isSafeHref('  JaVaScRiPt:alert(1)')).toBe(false);
    expect(isSafeHref('data:text/html;base64,AAAA')).toBe(false);
    expect(isSafeHref('//evil.example/x')).toBe(false);
    expect(isSafeHref('vbscript:x')).toBe(false);
  });

  it('draws a link to an unsafe address as its text, and a link away from the application in a tab of its own', () => {
    const page = draw('[bad](javascript:alert(1)) and [good](https://example.org/x)');
    expect(page.querySelectorAll('a')).toHaveLength(1);
    const link = page.querySelector('a');
    expect(link?.getAttribute('href')).toBe('https://example.org/x');
    expect(link?.getAttribute('target')).toBe('_blank');
    expect(link?.getAttribute('rel')).toBe('noopener noreferrer');
    expect(page.textContent).toContain('bad');
  });

  it('never fetches an image: it is a link to it', () => {
    const page = draw('![a chart](https://example.org/chart.png)');
    expect(page.querySelector('img')).toBeNull();
    expect(page.querySelector('a')?.getAttribute('href')).toBe('https://example.org/chart.png');
  });

  it('draws lists, a table, emphasis and code', () => {
    const page = draw(['- one', '- two', '', '| A | B |', '| - | - |', '| 1 | 2 |', '', '**bold** and `code`'].join('\n'));
    expect(page.querySelectorAll('li')).toHaveLength(2);
    expect(page.querySelectorAll('td')).toHaveLength(2);
    expect(page.querySelector('strong')?.textContent).toBe('bold');
    expect(page.querySelector('code')?.textContent).toBe('code');
  });
});

describe('a step in words', () => {
  const tool = (item: unknown): ToolItem => item as ToolItem;

  it('says a command as it was typed, with its exit', () => {
    const step = describeStep(tool(command('c', { command: 'npm test', output: '', exit: 1 })), t);
    expect(step.text).toBe('npm test');
    expect(step.command).toBe(true);
    expect(step.result).toBe('exit 1');
    expect(step.tone).toBe('danger');
    expect(stepFailed(tool(command('c', { command: 'npm test', output: '', exit: 1 })))).toBe(true);
    expect(stepFailed(tool(command('c', { command: 'npm test', output: '', exit: 0 })))).toBe(false);
  });

  it('says what the harness says a command was for, with the command under the pointer', () => {
    const step = describeStep(tool(command('c', { command: 'rg -n redirect src', output: '', exit: 0, actions: [{ type: 'search', query: 'redirect', path: 'src' }] })), t);
    expect(step.text).toBe('Searched for “redirect” in src');
    expect(step.command).toBe(false);
    expect(step.hint).toBe('rg -n redirect src');
  });

  it('says what a change did and what it came to', () => {
    const one = describeStep(tool(fileChange('e', { changes: [change('src/a.ts', '@@ -1 +1 @@\n-a\n+b')] })), t);
    expect(one.text).toBe('Edited src/a.ts');
    expect(one.result).toBe('+1 −1');
    const many = describeStep(tool(fileChange('e', { changes: [change('a', '+x'), change('b', '+y')] })), t);
    expect(many.text).toBe('Edited 2 files');
    const made = describeStep(tool(fileChange('e', { changes: [change('src/new.ts', '+x', { change: 'add' })] })), t);
    expect(made.text).toBe('Created src/new.ts');
  });

  it('says what was read, with the lines, and falls back to the title of a tool it has no words for', () => {
    expect(describeStep(tool(fileRead('r', { path: 'src/a.ts', content: 'x', from: 40, to: 120 })), t).text).toBe('Read src/a.ts · lines 40–120');
    expect(describeStep(tool(otherTool('o', { name: 'weird_tool' })), t).text).toBe('weird_tool');
  });
});
