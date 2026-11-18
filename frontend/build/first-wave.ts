/** Preloads the frame while the verified reader selects the first vocabulary. */
import { readFileSync } from 'node:fs';
import { Buffer } from 'node:buffer';
import { resolve } from 'node:path';
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
}

/** The files of the first wave that the entry imports dynamically, by built file name. */
export function firstWaveFiles(bundle: Readonly<Record<string, BuiltChunk>>): string[] {
  return dependencyFiles(bundle, Object.values(bundle).filter((item) => item.type === 'chunk' && (item.isEntry === true
    || /\/src\/(?:shell\/Shell\.tsx|features\/workbench\/pages\.ts)(?:\?|$)/.test(item.facadeModuleId?.replaceAll('\\', '/') ?? ''))).map((item) => item.fileName));
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

export interface FirstScreenPlan { entry: string[]; common: string[]; pages: { pattern: string; files: string[] }[] }

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
  return { entry: dependencyFiles(bundle, Object.values(bundle).filter((item) => item.type === 'chunk' && item.isEntry === true).map((item) => item.fileName)),
    common: firstWaveFiles(bundle), pages: [...roots].sort(([one], [other]) => other.length - one.length || one.localeCompare(other))
    .map(([pattern, files]) => ({ pattern, files: dependencyFiles(bundle, [...files]) })) };
}

export function firstWavePlugin(): Plugin {
  let root = process.cwd();
  return {
    name: 'first-wave',
    apply: 'build',
    configResolved(config) { root = config.root; },
    generateBundle(_options, output) {
      const bundle = output as unknown as Record<string, BuiltChunk>;
      const plan = firstScreenPlan(bundle);
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
        const charset = /<meta\b[^>]*charset[^>]*>/i;
        return { html: charset.test(html) ? html.replace(charset, (tag) => `${tag}\n${starter}`) : html.replace(/<head\b[^>]*>/i, (tag) => `${tag}\n${starter}`), tags: [] };
      },
    },
  };
}
