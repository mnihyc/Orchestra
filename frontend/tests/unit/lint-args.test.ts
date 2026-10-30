/** Explicit lint arguments reach the original ESLint command instead of being silently ignored. */
// @vitest-environment node
import { expect, it, vi } from 'vitest';

const fake = vi.hoisted(() => ({ spawn: vi.fn(), discover: vi.fn(), batches: vi.fn() }));
vi.mock('node:child_process', async (original) => {
  const actual = await original<typeof import('node:child_process')>();
  return { ...actual, default: actual, spawnSync: fake.spawn };
});
vi.mock('../../build/lint-plan.ts', () => ({ lintableFiles: fake.discover, lintBatches: fake.batches }));

it('passes explicit options to ESLint with the original directory argument', async () => {
  const previous = process.argv;
  const exit = vi.spyOn(process, 'exit').mockImplementation((code) => { throw new Error(`exit ${String(code)}`); });
  try {
    process.argv = [...previous.slice(0, 2), '--version'];
    fake.spawn.mockReturnValue({ status: 0, error: undefined });
    await expect(import('../../build/lint.ts')).rejects.toThrow('exit 0');
    expect(fake.spawn).toHaveBeenCalledExactlyOnceWith(process.execPath, expect.arrayContaining(['.', '--version']), expect.objectContaining({ stdio: 'inherit' }));
    expect(fake.discover).not.toHaveBeenCalled();
  } finally {
    process.argv = previous;
    exit.mockRestore();
  }
});

it('checks the remaining batches and fails when any batch fails', async () => {
  vi.resetModules(); fake.spawn.mockReset(); fake.discover.mockReset(); fake.batches.mockReset();
  const previous = process.argv;
  const previousExitCode = process.exitCode;
  try {
    process.argv = previous.slice(0, 2);
    process.exitCode = 0;
    fake.discover.mockResolvedValue(['src/a.ts', 'src/b.ts', 'tests/e2e/c.ts']);
    fake.batches.mockReturnValue([['src/a.ts'], ['src/b.ts'], ['tests/e2e/c.ts']]);
    fake.spawn.mockReturnValueOnce({ status: 0, error: undefined })
      .mockReturnValueOnce({ status: 1, signal: null, error: undefined })
      .mockReturnValueOnce({ status: 0, error: undefined });
    await import('../../build/lint.ts');
    expect(fake.spawn).toHaveBeenCalledTimes(3);
    expect(process.exitCode).toBe(1);
  } finally {
    process.argv = previous;
    process.exitCode = previousExitCode;
  }
});
