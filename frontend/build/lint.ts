/** Lint the entire configured tree in separate processes to release typed lint state. */
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { lintableFiles, lintBatches } from './lint-plan.ts';

const root = fileURLToPath(new URL('..', import.meta.url));
const args = process.argv.slice(2);
if (args.length > 0) {
  // Preserve ESLint's original CLI behavior for explicitly supplied arguments.
  const result = spawnSync(process.execPath, ['--max-old-space-size=8192', path.join(root, 'node_modules/eslint/bin/eslint.js'), '.', ...args], { cwd: root, stdio: 'inherit' });
  if (result.error !== undefined) throw result.error;
  process.exit(result.status ?? 1);
}
const files = await lintableFiles(root);
const batches = lintBatches(files);
if (batches.length === 0) throw new Error('ESLint found no configured files');
process.stdout.write(`Linting ${files.length} files in ${batches.length} batches\n`);
let failed = false;
for (const [index, batch] of batches.entries()) {
  process.stdout.write(`Lint batch ${index + 1}/${batches.length}: ${batch.length} files\n`);
  const result = spawnSync(process.execPath, ['--max-old-space-size=8192', path.join(root, 'node_modules/eslint/bin/eslint.js'), ...batch], { cwd: root, stdio: 'inherit' });
  if (result.error !== undefined) {
    process.stderr.write(`Could not start ESLint: ${result.error.message}\n`);
    failed = true;
  } else if (result.status !== 0) {
    process.stderr.write(`Lint batch ${index + 1} failed${result.signal === null ? '' : ` (${result.signal})`}\n`);
    failed = true;
  }
}
if (failed) process.exitCode = 1;
else process.stdout.write('All lint batches passed\n');
