import { expect, it } from 'vitest';
import { selectionEffort, selectionWithEffort } from '@/features/composer/modelSelection';

it('keeps absent, explicit none and native effort aliases distinct', () => {
  expect(selectionEffort({})).toBeUndefined(); expect(selectionEffort({ effort: null })).toBeNull();
  expect(selectionEffort({ reasoning: { effort: 'high' } })).toBe('high');
  expect(selectionEffort({ output_config: { effort: 'low' } })).toBe('low');
});
it('changes only effort aliases while preserving declared and extension siblings', () => {
  const preset = { reasoning: { effort: 'low', summary: 'brief' }, output_config: { effort: 'low', format: 'text' }, reasoning_effort: 'low', temperature: 0.2 };
  expect(selectionWithEffort(preset, 'high')).toEqual({ reasoning: { summary: 'brief' }, output_config: { format: 'text' }, temperature: 0.2, effort: 'high' });
  expect(preset.reasoning.effort).toBe('low');
});


it('reads Gemini thinking and changes it without losing other generation options', () => {
  const options = { generation_config: { thinking_level: 'high', temperature: 0.2 } };
  expect(selectionEffort(options)).toBe('high');
  expect(selectionWithEffort(options, 'low')).toEqual({ generation_config: { temperature: 0.2 }, effort: 'low' });
});
