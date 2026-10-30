/** Lint batches cover every configured file without combining the two TypeScript projects. */
// @vitest-environment node
import { expect, it } from 'vitest';
import { ARGUMENT_CHARS_PER_BATCH, FILES_PER_BATCH, lintBatches } from '../../build/lint-plan.ts';

it('keeps every file once while bounding batch and command size', () => {
  const files = [
    ...Array.from({ length: 801 }, (_, n) => `src/${'long-name-'.repeat(8)}${n}.ts`),
    ...Array.from({ length: 801 }, (_, n) => `tests/e2e/example-${n}.ts`),
  ];
  const batches = lintBatches(files);
  expect(batches.flat().sort()).toEqual([...files].sort());
  for (const batch of batches) {
    expect(batch.length).toBeLessThanOrEqual(FILES_PER_BATCH);
    expect(batch.reduce((size, file) => size + file.length + 1, 0)).toBeLessThanOrEqual(ARGUMENT_CHARS_PER_BATCH);
    expect(batch.every((file) => file.startsWith('src/')) || batch.every((file) => file.startsWith('tests/e2e/'))).toBe(true);
  }
});
