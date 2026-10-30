/** A command preview never loses the rest of its first line or subsequent lines. */
import { expect, it } from 'vitest';
import { describeStep, type ToolItem } from '@/features/timeline/titles';
import { i18n } from '@/i18n';
import { command } from '../doubles/thread-rows';

it('keeps a long multi-line command whole in its hint while its row stays short', () => {
  const original = `printf '${'a long literal argument '.repeat(20)}'\nprintf 'the last line'`;
  const item = command('long-command', { command: original, output: '', exit: 0 }) as ToolItem;
  const title = describeStep(item, i18n.t.bind(i18n));
  expect(title.text).toContain('…');
  expect(title.text.length).toBeLessThan(original.length);
  expect(title.hint).toContain(original);
  expect(title.hint).toContain('the last line');
});
