/** Discover the files ESLint accepts and divide them into bounded, project-local batches. */
import { readdir } from 'node:fs/promises';
import path from 'node:path';
import { ESLint } from 'eslint';

// Limiting files bounds ESLint's simultaneous reads and retained results. Keeping paths below
// 24,000 characters also leaves room for the executable and flags on Windows' command line.
export const FILES_PER_BATCH = 400;
export const ARGUMENT_CHARS_PER_BATCH = 24_000;

/** The same configured files as a directory lint, including hidden files and excluding ignored paths. */
export async function lintableFiles(root: string): Promise<string[]> {
  const eslint = new ESLint({ cwd: root });
  const files: string[] = [];
  const visit = async (directory: string): Promise<void> => {
    const entries = await readdir(path.join(root, directory), { withFileTypes: true });
    for (const entry of entries) {
      // Both are ESLint's default directory exclusions; do not traverse the linked dependencies.
      if (entry.name === 'node_modules' || entry.name === '.git') continue;
      const relative = path.join(directory, entry.name);
      if (entry.isDirectory()) await visit(relative);
      else if ((entry.isFile() || entry.isSymbolicLink()) && !await eslint.isPathIgnored(relative)) files.push(relative);
    }
  };
  await visit('');
  return files.sort();
}

function projectOf(file: string): 'app' | 'node' {
  const normalized = file.split(path.sep).join('/');
  return normalized.startsWith('src/') || normalized.startsWith('tests/unit/') || normalized.startsWith('tests/component/') || normalized === 'tests/setup.ts' ? 'app' : 'node';
}

/** Keep each file once, with the two TypeScript projects in separate processes. */
export function lintBatches(files: readonly string[]): string[][] {
  const batches: string[][] = [];
  for (const project of ['app', 'node'] as const) {
    let batch: string[] = [];
    let length = 0;
    for (const file of files.filter((item) => projectOf(item) === project)) {
      // A single long file name is passed on its own; do not silently drop it.
      if (batch.length > 0 && (batch.length >= FILES_PER_BATCH || length + file.length + 1 > ARGUMENT_CHARS_PER_BATCH)) {
        batches.push(batch); batch = []; length = 0;
      }
      batch.push(file);
      length += file.length + 1;
    }
    if (batch.length > 0) batches.push(batch);
  }
  return batches;
}
