/** What a sub-agent's row says of it is read from its own items: the first line that says something, its tool uses, what it does now. */
import { describe, expect, it } from 'vitest';

import { plainLine, subagentLine, subagentTask, subagentToolCount } from '@/features/timeline/subagents';
import { describeStep, type ToolItem } from '@/features/timeline/titles';
import type { Item } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { agentMessage, command, reasoning } from '../doubles/thread-rows';

function view(items: Item[], hasNewer = false) {
  const streams = new Map<string, string[]>();
  for (const item of items) streams.set(item.stream, [...(streams.get(item.stream) ?? []), item.id]);
  return { items: new Map(items.map((item) => [item.id, item])), streams, hasNewer };
}

describe('the first line of a reply in plain words', () => {
  it.each([
    ['## Findings\n\nThe **login flow** is in `src/login.ts`.', 'The login flow is in src/login.ts.'],
    ['- `web/` (directory)\n- `.orch/`', 'web/ (directory)'],
    ['1. Read [the guide](https://example.com/guide) first', 'Read the guide first'],
    ['```sh\nls -la\n```\nThe listing is above.', 'The listing is above.'],
    ['| file | size |\n|---|---|\n| a.ts | 10 |', 'file | size'],
    ['> quoted\nplain', 'quoted'],
    ['---\n\n   \n**Done.**', 'Done.'],
    ['- [x] checked item', 'checked item'],
    ['', ''],
  ])('reads %j as %j', (text, line) => {
    expect(plainLine(text)).toBe(line);
  });

  it('keeps the underscores and stars inside names', () => {
    expect(plainLine('Edited snake_case_name and a*b')).toBe('Edited snake_case_name and a*b');
  });
});

describe('a sub-agent read from its own items', () => {
  const ls = command('ls', { stream: 'sub:one', command: 'ls -la', output: 'a', exit: 0 });
  const grep = command('grep', { stream: 'sub:one', command: 'grep -rn login', status: 'in_progress' });
  const answer = agentMessage('answer', '# Result\nNo README here.', { stream: 'sub:one' });
  const other = command('other', { stream: 'sub:two', command: 'find .', exit: 0 });

  it('counts its own tool uses only, and none where the page does not hold the end of the thread', () => {
    expect(subagentToolCount(view([ls, grep, answer, other]), 'sub:one')).toBe(2);
    expect(subagentToolCount(view([ls, grep, answer, other], true), 'sub:one')).toBeNull();
    expect(subagentToolCount(view([ls]), '')).toBeNull();
  });

  it('says its newest step while it runs and the first line of its last message once it is over', () => {
    expect(subagentLine(view([ls, grep]), 'sub:one', true, i18n.t)).toEqual({ kind: 'activity', text: describeStep(grep as ToolItem, i18n.t).text });
    expect(subagentLine(view([ls, reasoning('thought', 'Hmm', { stream: 'sub:one' })]), 'sub:one', true, i18n.t)).toEqual({ kind: 'activity', text: i18n.t('thread.now.thinking') });
    expect(subagentLine(view([ls, answer, grep]), 'sub:one', false, i18n.t)).toEqual({ kind: 'result', text: 'No README here.' });
    expect(subagentLine(view([ls]), 'sub:one', false, i18n.t)).toBeNull();
  });

  it('names it by its name, else the first line of what it was asked', () => {
    const tool = { kind: 'subagent' as const, child_stream: 'sub:one', name: null, role: 'Explore', prompt: '**Find** the login flow.\nThen report.', state: 'running' as const };
    expect(subagentTask(tool)).toBe('Find the login flow.');
    expect(subagentTask({ ...tool, name: ' Login finder ' })).toBe('Login finder');
    expect(subagentTask({ ...tool, prompt: null })).toBeNull();
  });
});

describe('a command whose line has not arrived yet', () => {
  it('is told by the title its program gave it, never as a bare prompt mark', () => {
    const pending = command('pending', { command: '', status: 'in_progress' });
    if (pending.kind !== 'tool_call') throw new Error('The fixture is a tool call');
    expect(describeStep({ ...pending, payload: { ...pending.payload, title: 'Bash' } }, i18n.t)).toMatchObject({ text: 'Bash', command: false });
    expect(describeStep({ ...pending, payload: { ...pending.payload, title: '' } }, i18n.t)).toMatchObject({ text: i18n.t('timeline.step.commandPending'), command: false });
  });
});

describe('what a program that ended wrote', () => {
  it('stands a line each: how it ended, then the lines its runner joined', async () => {
    const { programLines } = await import('@/features/timeline/FailureAlert');
    expect(programLines('harness_error: exited with status 1: ⚠ connectors are disabled · Unset it |  | Error: sandbox required but unavailable |   sandbox.failIfUnavailable is set |'))
      .toBe('exited with status 1\n⚠ connectors are disabled · Unset it\nError: sandbox required but unavailable\nsandbox.failIfUnavailable is set');
    expect(programLines('was ended by signal 9: Killed')).toBe('was ended by signal 9\nKilled');
    expect(programLines('Something else | with a pipe')).toBe('Something else | with a pipe');
  });
});
