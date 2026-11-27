/** A change form follows the driver's live declaration and warns only against known measured use. */
import { describe, expect, it } from 'vitest';
import { memoryBelowUse } from '@/features/settings/runtimeEffects';
describe('the running unit change form', () => {
  it('compares the memory draft to current use, without inventing a reading or blocking the save', () => {
    expect(memoryBelowUse({ memory_mb: 32 }, 64 * 1024 * 1024)).toBe(true);
    expect(memoryBelowUse({ memory_mb: 64 }, 64 * 1024 * 1024)).toBe(false);
    expect(memoryBelowUse({ memory_mb: 128 }, 64 * 1024 * 1024)).toBe(false);
    expect(memoryBelowUse({ memory_mb: 32 }, null)).toBe(false);
    expect(memoryBelowUse({}, 64 * 1024 * 1024)).toBe(false);
  });
});
