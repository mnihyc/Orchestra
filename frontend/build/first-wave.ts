/** Preloads the frame while the verified reader selects the first vocabulary. */
import { readFileSync } from 'node:fs';
import { Buffer } from 'node:buffer';
import { resolve } from 'node:path';
import { gzipSync } from 'node:zlib';
import ts from 'typescript';
import type { Plugin } from 'vite';

/** The members of a built chunk that this reads. */
export interface BuiltChunk {
  type: string;
  fileName: string;
  name?: string;
  facadeModuleId?: string | null;
  moduleIds?: readonly string[];
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
  application?: { file: string; styles: string[] };
  pages: { pattern: string; files: string[] }[];
}

/** A bare side-effect facade adds no execution when its implementation already imports everything. */
function applicationEntry(bundle: Readonly<Record<string, BuiltChunk>>, entry: readonly string[]) {
  const chunks = Object.values(bundle).filter((item) => item.type === 'chunk');
  const isApplication = (id: string) => /\/src\/application\.tsx(?:\?|$)/.test(id.replaceAll('\\', '/'));
  const facade = chunks.find((item) => isApplication(item.facadeModuleId ?? ''))
    ?? chunks.find((item) => item.moduleIds?.some(isApplication));
  if (facade === undefined) return undefined;
  let selected = facade;
  const source = ts.createSourceFile(facade.fileName, facade.code ?? '', ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
  if (source.statements.length > 0 && source.statements.every((node) => ts.isImportDeclaration(node) && node.importClause === undefined)) {
    const candidates = (facade.imports ?? []).filter((file) => !entry.includes(file)).filter((file) => {
      const closure = new Set(dependencyFiles(bundle, [file]));
      return (facade.imports ?? []).every((dependency) => closure.has(dependency));
    });
    if (candidates.length === 1) selected = chunks.find((item) => item.fileName === candidates[0]) ?? facade;
  }
  // Include the facade's styles too: a Vite stylesheet is an effect even without a JS statement.
  return { file: selected.fileName, styles: entryTransfer(bundle, dependencyFiles(bundle, [facade.fileName])).entryStyles,
    redundant: selected === facade ? undefined : facade.fileName };
}

function entryTransfer(bundle: Readonly<Record<string, BuiltChunk>>, entry: readonly string[]): { entryStyles: string[]; entryGzipBytes: number } {
  const files = new Map(Object.values(bundle).map((item) => [item.fileName, item]));
  const styles = new Set<string>();
  for (const file of entry) for (const css of files.get(file)?.viteMetadata?.importedCss ?? []) styles.add(css);
  let bytes = 0;
  for (const file of [...entry, ...styles]) {
    const item = files.get(file);
    const source = item?.type === 'chunk' ? item.code : item?.source;
    if (source !== undefined) bytes += gzipSync(source, { level: 1 }).byteLength;
  }
  return { entryStyles: [...styles].sort(), entryGzipBytes: bytes };
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
    ...(application === undefined ? {} : { application: { file: application.file, styles: application.styles } }),
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
      const thread = plan.pages.find((page) => page.pattern.startsWith('^/thread/'));
      const files = new Set([...plan.common, ...(thread?.files ?? [])]);
      // Keep the real emitted graph in the build log, including any facade or style that still needs a file.
      this.warn(`Initial thread script graph: ${JSON.stringify(Object.values(bundle).filter((item) => files.has(item.fileName)).map((item) => ({
        file: item.fileName, facade: item.facadeModuleId?.replaceAll('\\', '/').split('/src/')[1] ?? null,
        dynamic: item.isDynamicEntry === true, bytes: Buffer.byteLength(item.code ?? '', 'utf8'), imports: item.imports ?? [],
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
        const starter = `<script nonce="__ORCH_CSP_NONCE__" data-orch-startup>${program}(window,${JSON.stringify(plan)});</script>`;
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
