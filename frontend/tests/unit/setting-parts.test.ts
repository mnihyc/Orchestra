/** Own settings never copy ancestors, and supplied contributions preserve their order and lock semantics. */
import { describe, expect, it } from 'vitest';
import { editableValue, inheritedParts } from '@/features/settings/settingParts';
import { shapeOf } from '@/features/settings/settingsModel';
import { resolvedParts } from '../doubles/day100-settings';
import type { SettingPart, ResolvedSetting as SettingWithParts } from '@/generated/api-types';

const target = { kind: 'node', id: 'child' } as const;
const parent: SettingPart = { source: { scope: 'org', node: { id: 'parent', kind: 'org', name: 'Team', redacted: false } }, value: 'From the team', locked: false, ignored: false };
const own: SettingPart = { source: { scope: 'project', node: { id: 'child', kind: 'project', name: 'Web', redacted: false } }, value: 'Own text', locked: false, ignored: false };
const setting: SettingWithParts = { key: 'agent.instructions', value: 'From the team\n\nOwn text', own: { value: 'Own text', locked: false, revision: 1 }, parts: [parent, own], source: own.source, locked_by: null, writable: true };

describe('a field which joins settings', () => {
  it('edits only its stored part and keeps every ancestor read-only in order', () => {
    expect(editableValue({ merge: 'append' }, setting, shapeOf({ type: 'string' }))).toBe('Own text');
    expect(inheritedParts(setting, target)).toEqual([parent]);
  });

  it.each(['append', 'union', 'intersect', 'map', 'min'] as const)('an absent %s part never takes the effective result as its draft', (merge) => {
    const shape = shapeOf(merge === 'append' ? { type: 'string' } : merge === 'min' ? { type: 'integer', minimum: 1 } : merge === 'map' ? { type: 'object', additionalProperties: { type: 'string' } } : { type: 'array', items: { type: 'string' } });
    const empty = editableValue({ merge }, { ...setting, own: null, value: 'Ancestor' }, shape);
    expect(empty).toEqual(merge === 'append' || merge === 'min' ? '' : merge === 'map' ? {} : []);
  });

  it('retains an explicitly empty own value rather than falling back to an ancestor', () => {
    expect(editableValue({ merge: 'append' }, { ...setting, own: { value: '', locked: false, revision: 2 } }, shapeOf({ type: 'string' }))).toBe('');
  });

  it('does not guess an ancestor from the resolved value when only the own part is supplied', () => {
    expect(inheritedParts({ ...setting, parts: [own] }, target)).toEqual([]);
  });

  it('shows a historical duplicate as stored text, without guessing a repair', () => {
    const text = 'From the team\n\nFrom the team';
    expect(editableValue({ merge: 'append' }, { ...setting, own: { value: text, revision: 1, locked: false } }, shapeOf({ type: 'string' }))).toBe(text);
  });
});

describe('the setting double resolves contributions', () => {
  it('repeated own saves cannot append an ancestor twice', () => {
    for (let attempt = 0; attempt < 3; attempt += 1) expect(resolvedParts({ merge: 'append' }, [parent, own]).value).toBe('From the team\n\nOwn text');
  });

  it('keeps an ignored own part visible under a higher lock', () => {
    expect(resolvedParts({ merge: 'append' }, [{ ...parent, locked: true }, own])).toMatchObject({ value: 'From the team', parts: [{ ignored: false }, { ignored: true }] });
  });

  it('combines unions, maps and narrowing values without replacing their ancestors', () => {
    expect(resolvedParts({ merge: 'union' }, [{ ...parent, value: ['a'] }, { ...own, value: ['a', 'b'] }]).value).toEqual(['a', 'b']);
    expect(resolvedParts({ merge: 'intersect' }, [{ ...parent, value: ['a', 'b'], locked: true }, { ...own, value: ['b'] }]).value).toEqual(['b']);
    expect(resolvedParts({ merge: 'map' }, [{ ...parent, value: { a: 1, b: 2 } }, { ...own, value: { b: 3 } }]).value).toEqual({ a: 1, b: 3 });
    expect(resolvedParts({ merge: 'min' }, [{ ...parent, value: 8, locked: true }, { ...own, value: 4 }]).value).toBe(4);
  });
});
