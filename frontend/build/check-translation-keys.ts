/** Reject authored missing translation keys before the ordinary compiler, without loading an application. */
import { readFileSync, readdirSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { present, translationKeys } from './translation-keys.ts';

const root = fileURLToPath(new URL('../src', import.meta.url));
const languages = ['en', 'zh-CN'] as const;
const readJson = (name: string): unknown => JSON.parse(readFileSync(resolve(root, 'i18n', name), 'utf8'));
const ordinary = new Map(languages.map((language) => [language, readJson(`${language}.json`)] as const));
const development = new Map(languages.map((language) => [language, readJson(`dev-${language}.json`)] as const));
function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) return entry.name === 'generated' ? [] : sources(path);
    return /\.tsx?$/u.test(entry.name) && !entry.name.endsWith('.d.ts') && entry.name !== 'routeTree.gen.ts' ? [path] : [];
  });
}
const failures: string[] = [];
const templates = new Set<string>();
let count = 0;
const files = sources(root);
for (const file of files) {
  const path = relative(root, file).replaceAll('\\', '/');
  for (const used of translationKeys(readFileSync(file, 'utf8'), file, true)) {
    count += 1;
    if (used.pattern !== undefined) templates.add(`${path}:${used.line} ${used.key}`);
    for (const language of languages) {
      const exists = present(ordinary.get(language), used, language)
        || path.startsWith('routes/dev/') && used.key.startsWith('dev.') && present(development.get(language), used, language);
      if (!exists) failures.push(`${path}:${used.line} ${language}:${used.key}`);
    }
  }
}
if (templates.size > 0) process.stdout.write(`Template translation keys (${templates.size}):\n${[...templates].join('\n')}\n`);
if (failures.length > 0) {
  process.stderr.write(`${failures.join('\n')}\n`);
  process.exitCode = 1;
} else process.stdout.write(`Translation keys: ${count} authored uses in ${files.length} files, both languages\n`);
