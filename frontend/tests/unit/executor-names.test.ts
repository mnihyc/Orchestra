/** Executor labels use their declarations and do not infer a name from an identifier. */
import { expect, it } from 'vitest';
import { declaredExecutorTitle, executorTitle } from '@/features/resources/executorNames';

it('uses the selected tool set and the reader’s language', () => {
  const reference = { kind: 'research-exec', title: { en: 'Research executor', 'zh-CN': '研究执行器' },
    tool_set: { title: { en: 'Research tools', 'zh-CN': '研究工具' } } };
  expect(executorTitle(reference, 'en')).toBe('Research tools');
  expect(executorTitle(reference, 'zh-CN')).toBe('研究工具');
  expect(executorTitle({ ...reference, tool_set: null }, 'zh-CN')).toBe('研究执行器');
  expect(executorTitle({ kind: 'research-exec' }, 'en')).toBeNull();
});

it('names an executor and its tool set from the program declaration', () => {
  const program = { executors: [{ kind: 'research-exec', title: { en: 'Research executor', 'zh-CN': '研究执行器' },
    tool_sets: [{ name: 'research-tools', title: { en: 'Research tools', 'zh-CN': '研究工具' } }] }] };
  expect(declaredExecutorTitle(program, 'research-exec', null, 'en')).toBe('Research executor');
  expect(declaredExecutorTitle(program, 'research-exec', null, 'zh-CN')).toBe('研究执行器');
  expect(declaredExecutorTitle(program, 'research-exec', 'research-tools', 'zh-CN')).toBe('研究工具');
  expect(declaredExecutorTitle(program, 'other-exec', null, 'en')).toBeNull();
  expect(declaredExecutorTitle(program, 'research-exec', 'other-tools', 'en')).toBeNull();
});
