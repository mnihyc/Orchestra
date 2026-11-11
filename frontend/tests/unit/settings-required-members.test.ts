/** New structured values include required initial fields without adding optional local contributions. */
import { expect, it } from 'vitest';
import { emptyOf, shapeOf } from '@/features/settings/settingsModel';

const shape = shapeOf({ type: 'object', additionalProperties: false, required: ['enabled', 'limits'], properties: {
  enabled: { type: 'boolean' },
  limits: { type: 'object', additionalProperties: false, required: ['count'], properties: {
    count: { type: 'integer', minimum: 2 }, note: { type: 'string' },
  } },
  optional: { type: 'object', additionalProperties: false, required: ['automatic'], properties: { automatic: { type: 'boolean' } } },
} });

it('seeds required members recursively while leaving optional objects absent', () => {
  expect(emptyOf(shape)).toEqual({ enabled: false, limits: { count: 2 } });
});

it('keeps separately enabled objects independent instead of sharing their initial members', () => {
  const first = emptyOf(shape) as { enabled: boolean; limits: { count: number } };
  const second = emptyOf(shape);
  first.limits.count = 9;
  expect(second).toEqual({ enabled: false, limits: { count: 2 } });
});
