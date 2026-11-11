import { expect, it } from 'vitest';
import { managedEffortForm } from '@/features/models/RequestOptions';

it('gives effort one owner while preserving native schema constraints and parameter siblings', () => {
  const schema = { type: 'object', properties: { reasoning: { type: 'object', properties: { effort: { enum: ['low', 'high'] }, summary: { enum: ['brief'] } } }, temperature: { type: 'number', minimum: 0, maximum: 1 } } };
  const value = { effort: 'high', reasoning: { summary: 'brief' }, temperature: 0.2 };
  const form = managedEffortForm(schema, value);
  expect(form.value).toEqual({ reasoning: { summary: 'brief' }, temperature: 0.2 });
  expect(form.schema).toBe(schema);
  expect(form.restore({ reasoning: { summary: 'brief' }, temperature: 0.5 })).toEqual({ effort: 'high', reasoning: { summary: 'brief' }, temperature: 0.5 });
  expect(value.temperature).toBe(0.2);
});
it('hides an effort-only object and restores an explicit none without inventing a value', () => {
  const form = managedEffortForm({ type: 'object', properties: { reasoning: { type: 'object', properties: { effort: { type: 'string' } } } } }, { reasoning: { effort: null } });
  expect(form.value).toEqual({}); expect(form.schema).toEqual({ type: 'object', properties: { reasoning: { type: 'object', properties: { effort: { type: 'string' } } } } });
  expect(form.restore({})).toEqual({ reasoning: { effort: null } });
});
it('keeps unrelated scalar members and replaces generated effort aliases with their dedicated owner', () => {
  const form = managedEffortForm(undefined, { reasoning: 'vendor-extension', effort: 'high' });
  expect(form.value).toEqual({ reasoning: 'vendor-extension' });
  expect(form.restore({ reasoning: 'vendor-extension', reasoning_effort: 'low', temperature: 0.2 })).toEqual({ reasoning: 'vendor-extension', effort: 'high', temperature: 0.2 });
});
