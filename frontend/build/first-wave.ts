/** Preloads the frame while the verified reader selects the first vocabulary. */
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { Buffer } from 'node:buffer';
import { posix, resolve } from 'node:path';
import { gzipSync } from 'node:zlib';
import ts from 'typescript';
import type { Plugin } from 'vite';
import { wordsOfBuild } from './vocabulary';

/** The members of a built chunk that this reads. */
export interface BuiltChunk {
  type: string;
  fileName: string;
  name?: string;
  facadeModuleId?: string | null;
  moduleIds?: readonly string[];
  modules?: Readonly<Record<string, { renderedLength: number }>>;
  imports?: readonly string[];
  isEntry?: boolean;
  isDynamicEntry?: boolean;
  code?: string;
  source?: string | Uint8Array;
  viteMetadata?: { importedCss?: ReadonlySet<string> };
}

// Code needed before the first shell fits a few cold TCP windows over the long-line profile.
// This build default counts gzip level 1, the stock edge's conservative compression level.
const FIRST_SHELL_GZIP_BYTES = 150 * 1024;
// On the 200 ms HTTP/1.1 profile, share the first two data flights with the addressed JSON.
const THREAD_GZIP_TARGET = 192 * 1024;
const FIRST_WORDS = 'virtual:orch-first-shell-words';

/** The files of the first wave that the entry imports dynamically, by built file name. */
export function firstWaveFiles(bundle: Readonly<Record<string, BuiltChunk>>): string[] {
  return dependencyFiles(bundle, Object.values(bundle).filter((item) => {
    if (item.type !== 'chunk') return false;
    if (item.isEntry === true) return true;
    const modules = item.facadeModuleId === null || item.facadeModuleId === undefined ? item.moduleIds ?? [] : [item.facadeModuleId];
    return modules.some((id) => /\/src\/(?:application\.tsx|shell\/Shell\.tsx|features\/workbench\/pages\.ts)(?:\?|$)/.test(id.replaceAll('\\', '/')));
  }).map((item) => item.fileName));
}

/** A whole static dependency wave, with a shared or cyclic dependency named once. */
function dependencyFiles(bundle: Readonly<Record<string, BuiltChunk>>, roots: readonly string[]): string[] {
  const chunks = new Map(Object.values(bundle).filter((item) => item.type === 'chunk').map((item) => [item.fileName, item]));
  const files = new Set<string>();
  const visit = (file: string) => {
    const item = chunks.get(file);
    if (item === undefined || files.has(file)) return;
    files.add(file);
    for (const dependency of item.imports ?? []) visit(dependency);
  };
  for (const root of roots) visit(root);
  return [...files].sort();
}

export interface FirstScreenPlan {
  entry: string[]; entryStyles: string[]; entryGzipBytes: number; common: string[];
  application?: { file: string; styles: string[]; initializers: string[] };
  pages: { pattern: string; files: string[] }[];
}

/** Recognize only imports, module markers and direct imported initializer calls, preserving call order. */
function entryCalls(source: ts.SourceFile, file: string): { file: string; name: string }[] | undefined {
  const bindings = new Map<string, { file: string; name: string }>();
  const calls: string[] = [];
  const call = (expression: ts.Expression): boolean => {
    if (ts.isParenthesizedExpression(expression)) return call(expression.expression);
    if (ts.isBinaryExpression(expression) && expression.operatorToken.kind === ts.SyntaxKind.CommaToken) return call(expression.left) && call(expression.right);
    if (!ts.isCallExpression(expression) || expression.questionDotToken !== undefined || expression.arguments.length !== 0 || !ts.isIdentifier(expression.expression)) return false;
    calls.push(expression.expression.text); return true;
  };
  for (const statement of source.statements) {
    if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
      const specifier = statement.moduleSpecifier.text;
      const imported = specifier.startsWith('.') ? posix.normalize(posix.join(posix.dirname(file), specifier)) : specifier;
      const clause = statement.importClause;
      if (clause?.name !== undefined) bindings.set(clause.name.text, { file: imported, name: 'default' });
      if (clause?.namedBindings !== undefined) {
        if (!ts.isNamedImports(clause.namedBindings)) return undefined;
        for (const binding of clause.namedBindings.elements) bindings.set(binding.name.text, { file: imported, name: binding.propertyName?.text ?? binding.name.text });
      }
    } else if (ts.isEmptyStatement(statement)) continue;
    else if (ts.isExportDeclaration(statement) && statement.moduleSpecifier === undefined && statement.exportClause !== undefined
      && ts.isNamedExports(statement.exportClause) && statement.exportClause.elements.length === 0) continue;
    else if (ts.isExpressionStatement(statement)) {
      if (ts.isStringLiteral(statement.expression) && statement.expression.text === 'use strict') continue;
      if (!call(statement.expression)) return undefined;
    } else return undefined;
  }
  const resolved: { file: string; name: string }[] = [];
  for (const name of calls) {
    const binding = bindings.get(name);
    if (binding === undefined) return undefined;
    resolved.push(binding);
  }
  return resolved;
}

/** A facade needs no transfer when its implementation supplies its imports and initializer calls. */
function applicationEntry(bundle: Readonly<Record<string, BuiltChunk>>, entry: readonly string[]) {
  const chunks = Object.values(bundle).filter((item) => item.type === 'chunk');
  const isApplication = (id: string) => /\/src\/application\.tsx(?:\?|$)/.test(id.replaceAll('\\', '/'));
  const facade = chunks.find((item) => isApplication(item.facadeModuleId ?? ''))
    ?? chunks.find((item) => item.moduleIds?.some(isApplication));
  if (facade === undefined) return undefined;
  let selected = facade;
  let initializers: string[] = [];
  const source = ts.createSourceFile(facade.fileName, facade.code ?? '', ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
  const calls = entryCalls(source, facade.fileName);
  if (source.statements.length > 0 && calls !== undefined) {
    const candidates = (facade.imports ?? []).filter((file) => !entry.includes(file)).filter((file) => {
      const closure = new Set(dependencyFiles(bundle, [file]));
      return (facade.imports ?? []).every((dependency) => closure.has(dependency)) && calls.every((item) => item.file === file);
    });
    if (candidates.length === 1) {
      selected = chunks.find((item) => item.fileName === candidates[0]) ?? facade;
      if (selected !== facade) initializers = calls.map((item) => item.name);
    }
  }
  // Include the facade's styles too: a Vite stylesheet is an effect even without a JS statement.
  return { file: selected.fileName, initializers, styles: entryTransfer(bundle, dependencyFiles(bundle, [facade.fileName])).entryStyles,
    source: facade.fileName, syntax: source.statements.map((statement) => ts.SyntaxKind[statement.kind]),
    redundant: selected === facade ? undefined : facade.fileName };
}

function entryTransfer(bundle: Readonly<Record<string, BuiltChunk>>, entry: readonly string[], blockingStyles: readonly string[] = []): { entryStyles: string[]; entryGzipBytes: number } {
  const files = new Map(Object.values(bundle).map((item) => [item.fileName, item]));
  const styles = new Set(blockingStyles);
  for (const file of entry) for (const css of files.get(file)?.viteMetadata?.importedCss ?? []) styles.add(css);
  let bytes = 0;
  for (const file of new Set([...entry, ...styles])) {
    const item = files.get(file);
    const source = item?.type === 'chunk' ? item.code : item?.source;
    if (source !== undefined) bytes += gzipSync(source, { level: 1 }).byteLength;
  }
  return { entryStyles: [...styles].sort(), entryGzipBytes: bytes };
}

/** The route modules each page pattern is drawn from, as module paths below the source directory. */
function pageModules(bundle: Readonly<Record<string, BuiltChunk>>): Map<string, Set<string>> {
  const modules = new Map<string, Set<string>>();
  for (const item of Object.values(bundle)) {
    if (item.type !== 'chunk') continue;
    for (const id of item.facadeModuleId === null || item.facadeModuleId === undefined ? item.moduleIds ?? [] : [item.facadeModuleId]) {
      const match = /\/src\/(routes\/([^?]+?)\.[jt]sx?)(?:\?|$)/.exec(id.replaceAll('\\', '/'));
      if (match?.[1] === undefined || match[2] === undefined) continue;
      const parts = match[2].split('/').filter((part) => part !== 'index');
      if (parts.some((part) => part.startsWith('_'))) continue;
      const path = parts.map((part) => part.startsWith('$') ? '[^/]+' : part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('/');
      const pattern = `^/${path}${path === '' ? '' : '/?'}$`;
      const held = modules.get(pattern) ?? new Set<string>();
      held.add(match[1]); modules.set(pattern, held);
    }
  }
  return modules;
}

/** Each page's first words, by the part the vocabulary build gave its route: the page's document may fetch them with itself. */
export function pageWords(bundle: Readonly<Record<string, BuiltChunk>>, words: { initial: string; routes: ReadonlyMap<string, string> }): { initial: string; pages: Record<string, string[]> } {
  const pages: Record<string, string[]> = {};
  for (const [pattern, modules] of pageModules(bundle)) {
    const parts = [...new Set([...modules].flatMap((module) => { const part = words.routes.get(module); return part === undefined ? [] : [part]; }))].sort();
    if (parts.length > 0) pages[pattern] = parts;
  }
  return { initial: words.initial, pages };
}

/** Route component chunks and their dependencies are known before the entry runs. */
export function firstScreenPlan(bundle: Readonly<Record<string, BuiltChunk>>): FirstScreenPlan {
  const roots = new Map<string, Set<string>>();
  for (const item of Object.values(bundle)) {
    if (item.type !== 'chunk') continue;
    // A copied shared factory can appear in several consumers; its facade identifies the actual entry.
    for (const id of item.facadeModuleId === null || item.facadeModuleId === undefined ? item.moduleIds ?? [] : [item.facadeModuleId]) {
      const match = /\/src\/routes\/([^?]+?)\.[jt]sx?(?:\?|$)/.exec(id.replaceAll('\\', '/'));
      if (match?.[1] === undefined) continue;
      const parts = match[1].split('/').filter((part) => part !== 'index');
      if (parts.some((part) => part.startsWith('_'))) continue;
      const path = parts.map((part) => part.startsWith('$') ? '[^/]+' : part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('/');
      const pattern = `^/${path}${path === '' ? '' : '/?'}$`;
      const files = roots.get(pattern) ?? new Set<string>();
      files.add(item.fileName); roots.set(pattern, files);
    }
  }
  const entry = dependencyFiles(bundle, Object.values(bundle).filter((item) => item.type === 'chunk' && item.isEntry === true).map((item) => item.fileName));
  const application = applicationEntry(bundle, entry);
  return { entry, ...entryTransfer(bundle, entry),
    ...(application === undefined ? {} : { application: { file: application.file, styles: application.styles, initializers: application.initializers } }),
    common: firstWaveFiles(bundle).filter((file) => file !== application?.redundant), pages: [...roots].sort(([one], [other]) => other.length - one.length || one.localeCompare(other))
    .map(([pattern, files]) => ({ pattern, files: dependencyFiles(bundle, [...files]) })) };
}

export function firstWavePlugin(): Plugin {
  let root = process.cwd();
  return {
    name: 'first-wave',
    configResolved(config) { root = config.root; },
    resolveId(id) { if (id === FIRST_WORDS) return `\0${id}`; },
    load(id) {
      if (id !== `\0${FIRST_WORDS}`) return;
      const words = Object.fromEntries(['en', 'zh-CN'].map((locale) => {
        const catalogue = JSON.parse(readFileSync(resolve(root, `src/i18n/${locale}.json`), 'utf8')) as { common: { tryAgain: string }; ui: { error: { partFailed: string } } };
        return [locale, { failed: catalogue.ui.error.partFailed, retry: catalogue.common.tryAgain }];
      }));
      return `export const words=${JSON.stringify(words)};`;
    },
    generateBundle(_options, output) {
      const bundle = output as unknown as Record<string, BuiltChunk>;
      const plan = firstScreenPlan(bundle);
      if (plan.entryGzipBytes > FIRST_SHELL_GZIP_BYTES) this.error(`The compressed first shell wave is ${plan.entryGzipBytes} bytes; keep entry and blocking styles within ${FIRST_SHELL_GZIP_BYTES} bytes (150 KiB).`);
      this.warn(`First shell compressed transfer: ${plan.entryGzipBytes} bytes, including ${plan.entryStyles.length} stylesheets; budget ${FIRST_SHELL_GZIP_BYTES} bytes.`);
      const application = applicationEntry(bundle, plan.entry);
      if (application !== undefined) this.warn(`Application entry: ${JSON.stringify({ source: application.source, selected: application.file,
        initializers: application.initializers, syntax: application.syntax })}`);
      const thread = plan.pages.find((page) => new RegExp(page.pattern).test('/thread/01000000-0000-7000-8000-000000000001'));
      const files = new Set([...plan.entry, ...plan.common, ...(thread?.files ?? [])]);
      const measured = entryTransfer(bundle, [...files], plan.application?.styles);
      this.warn(`Addressed thread compressed transfer: ${measured.entryGzipBytes} bytes (scripts: ${files.size}, stylesheets: ${measured.entryStyles.length}); two-flight target ${THREAD_GZIP_TARGET} bytes.`);
      // Keep the real emitted graph in the build log, including any facade or style that still needs a file.
      this.warn(`Initial thread script graph: ${JSON.stringify(Object.values(bundle).filter((item) => files.has(item.fileName)).map((item) => ({
        file: item.fileName, facade: item.facadeModuleId?.replaceAll('\\', '/').split('/src/')[1] ?? null,
        dynamic: item.isDynamicEntry === true, bytes: Buffer.byteLength(item.code ?? '', 'utf8'), imports: item.imports ?? [],
        // Keep this one-line report readable while identifying the largest remaining imports.
        largestModules: Object.entries(item.modules ?? {}).sort(([, a], [, b]) => b.renderedLength - a.renderedLength).slice(0, 5)
          .map(([id, module]) => ({ id: id.replaceAll('\\', '/').replace(root.replaceAll('\\', '/') + '/', ''), renderedLength: module.renderedLength })),
        styles: item.moduleIds?.some((id) => /\.(css|scss|sass|less|styl|stylus|pcss|postcss|sss)(\?|$)/.test(id)) ?? false,
      })))}`);
    },
    transformIndexHtml: {
      order: 'post',
      handler(html, context) {
        const bundle = context.bundle as unknown as Record<string, BuiltChunk> | undefined;
        if (bundle === undefined) return html;
        const plan = firstScreenPlan(bundle);
        if (plan.entryGzipBytes > FIRST_SHELL_GZIP_BYTES) throw new Error(`The compressed first shell wave exceeds ${FIRST_SHELL_GZIP_BYTES} bytes (150 KiB).`);
        const program = readFileSync(resolve(root, 'src/shell/startup.inline.js'), 'utf8');
        // Keep the preload scanner from filling HTTP/1.1 connections before the inline fetch runs.
        // The module still has its normal deferred execution and stylesheet ordering.
        html = html.replace(/<link\b(?=[^>]*\brel\s*=\s*["']modulepreload["'])[^>]*>/gi, '');
        html = html.replace(/<script\b(?=[^>]*\btype\s*=\s*["']module["'])([^>]*)>[\s\S]*?<\/script>/gi, (tag, attrs: string) => {
          const src = /\bsrc\s*=\s*(["'])(.*?)\1/i.exec(attrs)?.[2];
          if (src === undefined) return tag;
          if (src.startsWith('/') && !plan.entry.includes(src.slice(1))) plan.entry.push(src.slice(1));
          return `<script type="module" nonce="__ORCH_CSP_NONCE__" data-orch-entry>await import(${JSON.stringify(src)});</script>`;
        });
        plan.entry = dependencyFiles(bundle, plan.entry);
        const built = wordsOfBuild();
        const words = built === undefined ? undefined : pageWords(bundle, built);
        const starter = `<script nonce="__ORCH_CSP_NONCE__" data-orch-startup>${program}(window,${JSON.stringify(words === undefined ? plan : { ...plan, words })});</script>`;
        const fonts = Object.values(bundle).filter((item) => item.type === 'asset'
          && /^fonts\/(?:inter|jetbrains-mono)-latin-wght-normal-[^/]+\.woff2$/.test(item.fileName))
          .sort((one, other) => one.fileName.localeCompare(other.fileName))
          .map((item) => `<link rel="preload" as="font" type="font/woff2" href="/${item.fileName}" crossorigin="anonymous" fetchpriority="low" media="not all" data-orch-font>`).join('\n');
        const charset = /<meta\b[^>]*charset[^>]*>/i;
        return { html: charset.test(html) ? html.replace(charset, (tag) => `${tag}\n${starter}\n${fonts}`) : html.replace(/<head\b[^>]*>/i, (tag) => `${tag}\n${starter}\n${fonts}`), tags: [] };
      },
    },
  };
}

/** Pin a public copy of the finished document to this release of the browser worker. */
export function threadShellWorkerPlugin(): Plugin {
  let root = process.cwd();
  return {
    name: 'cached-thread-document',
    apply: 'build',
    enforce: 'post',
    configResolved(config) { root = config.root; },
    generateBundle: {
      order: 'post',
      handler(_options, bundle) {
        const document = bundle['index.html'];
        if (document?.type !== 'asset') this.error('The finished application document is missing.');
        const html = typeof document.source === 'string' ? document.source : Buffer.from(document.source).toString('utf8');
        // Only the sign-in document asks the edge for the public installation policy.
        const policy = '<template data-orch-login-meta><!--# include virtual="/internal/login-meta" set="login_meta" --><!--# echo var="login_meta" encoding="entity" --></template>';
        const starter = /<script\b[^>]*\bdata-orch-startup\b[^>]*>/;
        this.emitFile({ type: 'asset', fileName: 'login.html', source: html.replace(starter, (tag) => policy + tag) });
        const source = JSON.stringify(html);
        const digest = createHash('sha256').update(source).digest();
        const fileName = `assets/thread-shell-${digest.toString('hex')}.json`;
        this.emitFile({ type: 'asset', fileName, source });
        const worker = readFileSync(resolve(root, 'public/sw.js'), 'utf8');
        const asset = JSON.stringify({ url: '/' + fileName, integrity: 'sha256-' + digest.toString('base64') });
        this.emitFile({ type: 'asset', fileName: 'sw.js', source: worker.replace('const threadShell = null;', () => `const threadShell = ${asset};`) });
      },
    },
  };
}
