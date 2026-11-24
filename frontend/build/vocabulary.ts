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
  const complete = (held: Set<string>) => {
    for (const key of held) for (const locale of ['en', 'zh-CN'] as const) {
      const value = all[locale].get(key);
      const texts = typeof value === 'string' ? [value] : Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
      for (const text of texts) for (const match of text.matchAll(/\$t\(\s*([^,)\s]+)/g)) {
        const target = match[1];
        if (target === undefined) continue;
        if (all.en.has(target)) held.add(target);
        for (const member of prefixes.get(`${target}.`) ?? []) held.add(member);
        for (const member of plurals.get(target) ?? []) held.add(member);
      }
    }
  };
  for (const entry of ['src/application.tsx', 'src/shell/Shell.tsx']) {
    const graph = await entryGraph(entry, root);
    for (const name of graph.modules.keys()) for (const key of keys.get(join(src, name)) ?? []) initial.add(key);
  }
  const parts = new Map<string, Part>();
  const put = (held: Set<string>, name?: string) => {
    const words = { en: subset(all.en, held), 'zh-CN': subset(all['zh-CN'], held) };
    const id = name ?? hash(JSON.stringify(words));
    if (!parts.has(id)) parts.set(id, { keys: held, words });
    return id;
  };
  complete(initial); put(initial, 'initial');
  const moduleParts = new Map<string, string>();
  for (const target of targets) {
    const held = new Set<string>(), seen = new Set<string>(), queue = [target];
    for (let file = queue.pop(); file !== undefined; file = queue.pop()) {
      if (seen.has(file)) continue;
      seen.add(file);
      for (const key of keys.get(file) ?? []) if (!initial.has(key)) held.add(key);
      queue.push(...(deps.get(file) ?? []));
    }
    complete(held);
    for (const key of initial) held.delete(key);
    // The application initializes its own language after restoring the reader. Its import belongs
    // after the first shell paint and must not pull the catalogue runtime into the small entry.
    if (held.size === 0 || target === join(src, 'shell/pageFirstReads.ts') || target === join(src, 'application.tsx')) continue;
    const id = put(held);
    moduleParts.set(relative(src, target).replaceAll('\\', '/'), id);
  }
  return { parts, moduleParts };
}

export function vocabularyPlugin(): Plugin {
  let root: string, building = false, plan: Promise<VocabularyPlan>;
  let initialAsset = '', initialPart = '';
  return {
    name: 'first-use-vocabulary', enforce: 'post',
    configResolved(config) { root = config.root; building = config.command === 'build'; plan = vocabularyPlan(root); },
    async buildStart() {
      if (!building) return;
      for (const [part, value] of (await plan).parts) for (const locale of ['en', 'zh-CN'] as const) {
        const source = JSON.stringify(value.words[locale]);
        const address = part === 'initial' ? hash(JSON.stringify(value.words)) : part;
        const file = this.emitFile({ type: 'asset', fileName: `assets/words/${locale}-${address}.json`, source });
        if (part === 'initial' && locale === 'en') { initialAsset = file; initialPart = address; }
      }
    },
    resolveId(id) { if (id === MANIFEST || id.startsWith(PIECE)) return `\0${id}`; },
    async load(id) {
      if (id === `\0${MANIFEST}`) {
        const value = await plan;
        // The workbench selects a route by name. Every other lazy import already knows its part.
        const routes = [...value.moduleParts].filter(([name]) => name.startsWith('routes/'));
        const manifest = `export const moduleParts=${JSON.stringify(Object.fromEntries(routes))};`;
        if (building) {
          return manifest + `const base=new URL(import.meta.ROLLUP_FILE_URL_${initialAsset},import.meta.url);
            export async function readWords(locale,part) {
              const name=locale+'-'+(part==='initial'?${JSON.stringify(initialPart)}:part)+'.json';
              const response=await fetch(new URL(name,base));
              if(!response.ok) throw new Error('Vocabulary unavailable');
              return response.json();
            }`;
        }
        const loaders: string[] = [];
        for (const part of value.parts.keys()) for (const locale of ['en', 'zh-CN'] as const) {
          const key = `${locale}/${part}`;
          loaders.push(`${JSON.stringify(key)}:()=>import(${JSON.stringify(PIECE + key)}).then(m=>m.default)`);
        }
        return manifest + `const loaders={${loaders.join(',')}};export function readWords(locale,part){return loaders[locale+'/'+part]()}`;
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
        const part = value.moduleParts.get(name);
        if (part === undefined) continue;
        result = result.slice(0, item.start) + `Promise.all([${code.slice(item.start, item.end)},__orchWords(${JSON.stringify(part)})]).then(([module])=>module)` + result.slice(item.end);
        changed = true;
      }
      if (changed) return { code: `import {prepareWords as __orchWords} from '@/i18n/wordRuntime';\n${result}`, map: null };
    },
  };
}
