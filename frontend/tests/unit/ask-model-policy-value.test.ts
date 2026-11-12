import { expect, it } from 'vitest';
import { editableValue } from '@/features/settings/settingParts';
import { shapeOf } from '@/features/settings/settingsModel';
import type { ResolvedSetting } from '@/generated/api-types';

const value = [{ provider_id: 'api', model: 'one' }, { provider_id: 'api', model: 'two' }];
const inherited: ResolvedSetting = { key: 'ask.models', value, own: null, source: { scope: 'system', node: null }, locked_by: null, writable: true,
  parts: [{ source: { scope: 'system', node: null }, value, locked: false, ignored: false }] };
it('starts a narrowing list from its served effective entries even without an own contribution', () => {
  const shape = shapeOf({ type: 'array', uniqueItems: true, items: { type: 'object', additionalProperties: false,
    required: ['provider_id', 'model'], properties: { provider_id: { type: 'string' }, model: { type: 'string' } } } });
  expect(editableValue({ key: 'ask.models', merge: 'intersect' }, inherited, shape)).toBe(value);
});
it('keeps ordinary joining fields on their original own-part behavior', () => {
  expect(editableValue({ key: 'org.terms', merge: 'append' }, { ...inherited, key: 'org.terms', value: 'Inherited terms' }, { type: 'text', maxLength: null, pattern: null })).toBe('');
});
