/** @vitest-environment node */
/** The fake API's import entry loads in plain Node, without an alias loader, JSX support or a browser. */
import { spawnSync } from 'node:child_process';
import { expect, it } from 'vitest';

it('loads the import producer in the same plain Node runtime as the fake API', () => {
  const target = new URL('../doubles/people-import.ts',import.meta.url).href;
  const result = spawnSync(process.execPath,['--input-type=module','-e',`await import(${JSON.stringify(target)});`],{encoding:'utf8',timeout:5_000});
  expect(result.error,result.stderr).toBeUndefined();
  expect(result.status,result.stderr).toBe(0);
});
