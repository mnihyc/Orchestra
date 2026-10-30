/** Configuration edits keep reported descriptor defaults out of the overrides they write. */
import { describe, expect, it } from 'vitest';
import { descriptorOverrides, readObject } from '@/features/resources/jsonObject';

describe('public resource configuration', () => {
  it('accepts an object including an empty reset and rejects a broken value, array or null', () => {
    expect(readObject('{}')).toEqual({});
    expect(readObject('{"transport":"stdio","args":[]}')).toEqual({ transport: 'stdio', args: [] });
    for (const text of ['{', '[]', 'null', 'false', '"a program"']) expect(readObject(text)).toBeNull();
  });
  it('starts with only the members whose values came from an override', () => {
    expect(descriptorOverrides({ harness: { version: '1.0' }, capabilities: { steer: false }, parameters: [] }, ['capabilities'])).toEqual({ capabilities: { steer: false } });
    expect(descriptorOverrides({ harness: { version: '1.0' } }, [])).toEqual({});
    expect(descriptorOverrides({}, ['not-reported'])).toEqual({});
  });
});
