/** Builds each first-use vocabulary from the same local import graph as its code. */
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { isAbsolute, join, relative } from 'node:path';
import ts from 'typescript';
import type { Plugin } from 'vite';
import { entryGraph, resolveImport, staticSpecifiers } from './module-graph';

type Words = Record<string, unknown>;
type Locale = 'en' | 'zh-CN';
interface Part { keys: Set<string>; words: Record<Locale, Words> }
export interface VocabularyPlan { parts: Map<string, Part>; moduleParts: Map<string, string> }
const MANIFEST = 'virtual:orch-vocabulary';
const PIECE = 'virtual:orch-words/';
const hash = (value: string): string => createHash('sha256').update(value).digest('hex');
const object = (value: unknown): value is Words => typeof value === 'object' && value !== null && !Array.isArray(value);

function leaves(words: Words, prefix = '', result = new Map<string, unknown>()): Map<string, unknown> {
  for (const [name, value] of Object.entries(words)) {
    const key = prefix + name;
    if (object(value)) leaves(value, `${key}.`, result); else result.set(key, value);
  }
  return result;
}

function subset(all: ReadonlyMap<string, unknown>, keys: ReadonlySet<string>): Words {
  const result: Words = {};
  for (const key of [...keys].sort()) {
    const path = key.split('.');
    const leaf = path.pop();
    if (leaf === undefined) continue;
    let at = result;
    for (const name of path) { const next = at[name]; if (!object(next)) at[name] = {}; at = at[name] as Words; }
    at[leaf] = all.get(key);
  }
  return result;
}

function imports(file: string, code: string): { start: number; end: number; specifier: string }[] {
  const source = ts.createSourceFile(file, code, ts.ScriptTarget.Latest, true, file.endsWith('x') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const found: { start: number; end: number; specifier: string }[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword) {
      const arg = node.arguments[0];
      if (arg !== undefined && ts.isStringLiteral(arg)) found.push({ start: node.getStart(source), end: node.end, specifier: arg.text });
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return found;
}

export async function vocabularyPlan(root: string): Promise<VocabularyPlan> {
  const src = join(root, 'src');
  const all: Record<Locale, Map<string, unknown>> = {
    en: leaves(JSON.parse(readFileSync(join(src, 'i18n/en.json'), 'utf8')) as Words),
    'zh-CN': leaves(JSON.parse(readFileSync(join(src, 'i18n/zh-CN.json'), 'utf8')) as Words),
  };
  const prefixes = new Map<string, Set<string>>();
  const plurals = new Map<string, Set<string>>();
  for (const key of all.en.keys()) {
    const path = key.split('.');
    path.pop();
    while (path.length > 0) {
      const prefix = path.join('.') + '.';
      const group = prefixes.get(prefix) ?? new Set<string>(); group.add(key); prefixes.set(prefix, group); path.pop();
    }
    const base = key.replace(/_(?:ordinal_)?(?:zero|one|two|few|many|other)$/, '');
    if (base !== key) { const group = plurals.get(base) ?? new Set<string>(); group.add(key); plurals.set(base, group); }
  }
  const files = readdirSync(src, { recursive: true, withFileTypes: true }).filter((entry) => entry.isFile() && /\.tsx?$/.test(entry.name) && !entry.name.endsWith('.d.ts'))
    .map((entry) => join(entry.parentPath, entry.name));
  const codes = new Map(files.map((file) => [file, readFileSync(file, 'utf8')]));
  const keys = new Map<string, Set<string>>();
  const deps = new Map<string, string[]>();
  const targets = new Set<string>();
  const local = (from: string, specifier: string) => {
    const path = specifier.split('?')[0] ?? specifier;
    if (path.startsWith('/src/')) return { file: join(root, path.slice(1)) };
    if (isAbsolute(path) && path.startsWith(src + '/')) return { file: path };
    return resolveImport(src, from, path);
  };
  for (const [file, code] of codes) {
    if (file.includes('/routes/') && code.includes('createFileRoute(')) targets.add(file);
    const source = ts.createSourceFile(file, code, ts.ScriptTarget.Latest, true, file.endsWith('x') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
    const used = new Set<string>();
    const include = (text: string, dynamic = false) => {
      if (all.en.has(text)) used.add(text);
      for (const key of plurals.get(text) ?? []) used.add(key);
      if (dynamic || text.includes('.')) for (const key of prefixes.get(text.endsWith('.') ? text : `${text}.`) ?? []) used.add(key);
    };
    const visit = (node: ts.Node): void => {
      if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) include(node.text);
      else if (ts.isTemplateExpression(node) && node.head.text.includes('.')) include(node.head.text, true);
      ts.forEachChild(node, visit);
    };
    visit(source); keys.set(file, used);
    deps.set(file, staticSpecifiers(file, code).flatMap((specifier) => {
      const target = resolveImport(src, file, specifier); return target !== null && 'file' in target ? [target.file] : [];
    }));
    for (const item of imports(file, code)) {
      const target = local(file, item.specifier);
      if (target !== null && 'file' in target && /\.tsx?$/.test(target.file)) targets.add(target.file);
    }
  }
  const initial = new Set<string>();
  for (const entry of ['src/main.tsx', 'src/shell/Shell.tsx']) {
    const graph = await entryGraph(entry, root);
    for (const name of graph.modules.keys()) for (const key of keys.get(join(src, name)) ?? []) initial.add(key);
  }
  const parts = new Map<string, Part>();
  const put = (name: string, held: Set<string>) => parts.set(name, { keys: held, words: { en: subset(all.en, held), 'zh-CN': subset(all['zh-CN'], held) } });
  put('initial', initial);
  const moduleParts = new Map<string, string>();
  for (const target of targets) {
    const held = new Set<string>(), seen = new Set<string>(), queue = [target];
    for (let file = queue.pop(); file !== undefined; file = queue.pop()) {
      if (seen.has(file)) continue;
      seen.add(file);
      for (const key of keys.get(file) ?? []) if (!initial.has(key)) held.add(key);
      queue.push(...(deps.get(file) ?? []));
    }
    if (held.size === 0 || target === join(src, 'shell/pageFirstReads.ts')) continue;
    const id = hash([...held].sort().join('\n'));
    if (!parts.has(id)) put(id, held);
    moduleParts.set(relative(src, target).replaceAll('\\', '/'), id);
  }
  return { parts, moduleParts };
}

export function vocabularyPlugin(): Plugin {
  let root: string, building = false, plan: Promise<VocabularyPlan>;
  const assets = new Map<string, string>();
  return {
    name: 'first-use-vocabulary', enforce: 'post',
    configResolved(config) { root = config.root; building = config.command === 'build'; plan = vocabularyPlan(root); },
    async buildStart() {
      if (!building) return;
      for (const [part, value] of (await plan).parts) for (const locale of ['en', 'zh-CN'] as const) {
        const source = JSON.stringify(value.words[locale]);
        assets.set(`${locale}/${part}`, this.emitFile({ type: 'asset', fileName: `assets/words/${locale}-${hash(source)}.json`, source }));
      }
    },
    resolveId(id) { if (id === MANIFEST || id.startsWith(PIECE)) return `\0${id}`; },
    async load(id) {
      if (id === `\0${MANIFEST}`) {
        const value = await plan;
        const loaders: string[] = [];
        for (const part of value.parts.keys()) for (const locale of ['en', 'zh-CN'] as const) {
          const key = `${locale}/${part}`;
          loaders.push(`${JSON.stringify(key)}:()=>${building
            ? `fetch(import.meta.ROLLUP_FILE_URL_${assets.get(key) ?? ''}).then(r=>{if(!r.ok)throw new Error('Vocabulary unavailable');return r.json()})`
            : `import(${JSON.stringify(PIECE + key)}).then(m=>m.default)`}`);
        }
        return `export const moduleParts=${JSON.stringify(Object.fromEntries(value.moduleParts))};export const loaders={${loaders.join(',')}};`;
      }
      if (id.startsWith(`\0${PIECE}`)) {
        const [locale, part] = id.slice(PIECE.length + 1).split('/');
        const words = (await plan).parts.get(part ?? '')?.words[locale as Locale];
        return `export default ${JSON.stringify(words ?? {})};`;
      }
    },
    async transform(code, id) {
      if (!id.startsWith(join(root, 'src') + '/') || id.includes('?') || !/\.tsx?$/.test(id) || id.includes('/i18n/')) return;
      const value = await plan;
      let result = code, changed = false;
      for (const item of imports(id, code).reverse()) {
        const path = item.specifier.split('?')[0] ?? item.specifier;
        const target = path.startsWith('/src/') ? { file: join(root, path.slice(1)) }
          : isAbsolute(path) && path.startsWith(join(root, 'src') + '/') ? { file: path } : resolveImport(join(root, 'src'), id, path);
        if (target === null || !('file' in target)) continue;
        const name = relative(join(root, 'src'), target.file).replaceAll('\\', '/');
        if (!value.moduleParts.has(name)) continue;
        result = result.slice(0, item.start) + `Promise.all([${code.slice(item.start, item.end)},__orchWords(${JSON.stringify(name)})]).then(([module])=>module)` + result.slice(item.end);
        changed = true;
      }
      if (changed) return { code: `import {prepareModuleWords as __orchWords} from '@/i18n/wordRuntime';\n${result}`, map: null };
    },
  };
}
