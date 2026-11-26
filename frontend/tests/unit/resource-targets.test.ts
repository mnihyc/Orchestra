/** The same typed identity keeps backend placement and extension data without depending on key order. */
import { expect, it } from 'vitest';
import { resourceTargetIdentity } from '@/lib/resourceTargets';
import type { BackendRef } from '@/generated/api-types';

const backend: BackendRef = { route: 'generation.delegation', offering: { provider: 'provider', offering: 'Example' }, config: { environment: 'first', nested: { a: 1, b: 2 } }, future: { codec: 'text', generation: 3 } };
it('retains an exact backend when nested JSON members are reordered', () => {
  const identity = (value: BackendRef) => resourceTargetIdentity({ type: 'single', resource: { kind: 'model', backend: value } });
  expect(identity(backend)).toBe(identity({ ...backend, config: { nested: { b: 2, a: 1 }, environment: 'first' }, future: { generation: 3, codec: 'text' } }));
});
it('keeps two placements and meaningful extension values as different choices', () => {
  const identity = (value: BackendRef) => resourceTargetIdentity({ type: 'single', resource: { kind: 'model', backend: value } });
  expect(identity(backend)).not.toBe(identity({ ...backend, config: { environment: 'second' } }));
  expect(identity(backend)).not.toBe(identity({ ...backend, future: { codec: 'image', generation: 3 } }));
  expect(resourceTargetIdentity({ type: 'pool', id: 'pool', kind: 'host' })).not.toBe(resourceTargetIdentity({ type: 'pool', id: 'pool', kind: 'model' }));
});
