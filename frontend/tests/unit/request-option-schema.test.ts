/** Protocol controls preserve raw constraints and distinguish no rule, false and an indirect schema. */
import { expect, it } from 'vitest';
import { optionProperty, setOptionProperty } from '@/features/models/requestOptionSchema';

it('changes only a selected nested property across protocols, native rules and opaque keywords', () => {
  const original = {
    'openai-responses': { type: 'object', additionalProperties: false, required: ['text'], 'x-custom': { stay: true }, properties: {
      text: { type: 'object', title: 'Authored text', required: ['format'], properties: { format: { const: { type: 'text' } }, verbosity: { enum: ['low', 'high'] } } },
      safety_identifier: false,
    } },
    'vendor-format': { type: 'object', custom: { raw: ['keep'] } }, native: { properties: { effort: { const: 'high' } } },
  };
  const snapshot = structuredClone(original);
  const rule = { type: 'string', enum: ['medium'], default: 'medium' };
  const edit = setOptionProperty(original, 'openai-responses', ['text', 'verbosity'], rule);
  expect(edit).toEqual({ ok: true, value: { ...original, 'openai-responses': { ...original['openai-responses'], properties: {
    ...original['openai-responses'].properties, text: { ...original['openai-responses'].properties.text, properties: {
      ...original['openai-responses'].properties.text.properties, verbosity: rule,
    } },
  } } } });
  expect(original).toEqual(snapshot);
});

it('keeps defaults, constants, null replacement, explicit false and required separate', () => {
  const doc = { chat: { type: 'object', additionalProperties: false, required: ['user'], properties: {
    user: { const: null, 'x-orch-replace': true }, metadata: false, store: { type: 'boolean', default: false },
  } } };
  expect(optionProperty(doc, 'chat', ['user'])).toEqual({ state: 'present', rule: { const: null, 'x-orch-replace': true }, required: true, closed: false });
  expect(optionProperty(doc, 'chat', ['metadata'])).toMatchObject({ state: 'present', rule: false });
  expect(optionProperty(doc, 'chat', ['unknown'])).toMatchObject({ state: 'absent', closed: true });
  const reset = setOptionProperty(doc, 'chat', ['store'], undefined);
  expect(reset).toEqual({ ok: true, value: { chat: { ...doc.chat, properties: { user: doc.chat.properties.user, metadata: false } } } });
  expect(setOptionProperty(doc, 'chat', ['store'], false)).toMatchObject({ ok: true, value: { chat: { properties: { store: false } } } });
});

it('leaves custom and blocked ancestors in raw JSON instead of replacing their meaning', () => {
  for (const rule of [false, { $ref: '#/$defs/meta' }, { anyOf: [{ type: 'object' }, { type: 'null' }] }, { type: 'array' }, { properties: [] }]) {
    const doc = { wire: { properties: { metadata: rule } } };
    const snapshot = structuredClone(doc);
    expect(setOptionProperty(doc, 'wire', ['metadata', 'user_id'], { type: 'string' }).ok).toBe(false);
    expect(originalJSON(doc)).toBe(originalJSON(snapshot));
    expect(['custom', 'blocked']).toContain(optionProperty(doc, 'wire', ['metadata', 'user_id']).state);
  }
});
const originalJSON = (value: unknown) => JSON.stringify(value);

it('does not turn a reset into a new declaration, and rejects malformed documents or empty paths', () => {
  const doc = { native: { properties: {} } };
  expect(setOptionProperty(doc, 'new-wire', ['user'], undefined)).toEqual({ ok: true, value: doc });
  expect(optionProperty(doc, 'new-wire', ['user'])).toMatchObject({ state: 'absent', closed: false });
  for (const malformed of [null, [], 'raw draft']) expect(setOptionProperty(malformed, 'wire', ['user'], false)).toEqual({ ok: false, reason: 'document' });
  expect(setOptionProperty(doc, 'wire', [], false)).toEqual({ ok: false, reason: 'path' });
  expect(setOptionProperty(doc, 'wire', [''], false)).toEqual({ ok: false, reason: 'path' });
});

it('keeps property names literal without changing a prototype', () => {
  const edit = setOptionProperty({}, '__proto__', ['constructor', '__proto__'], { type: 'string' });
  expect(edit.ok).toBe(true);
  if (!edit.ok) throw new Error('A literal JSON property was rejected');
  expect(Object.getPrototypeOf(edit.value)).toBe(Object.prototype);
  expect(Object.hasOwn(edit.value, '__proto__')).toBe(true);
  expect(optionProperty(edit.value, '__proto__', ['constructor', '__proto__'])).toMatchObject({ state: 'present', rule: { type: 'string' } });
});
