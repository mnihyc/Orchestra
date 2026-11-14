/** Preloads the frame while the verified reader selects the first vocabulary. */
import type { HtmlTagDescriptor, Plugin } from 'vite';

/** The members of a built chunk that this reads. */
export interface BuiltChunk {
  type: string;
  fileName: string;
  name?: string;
  facadeModuleId?: string | null;
  moduleIds?: readonly string[];
}

/** The chunk the build names `shell`: the workbench's frame and what only it needs (see `vite.config.ts`). */
const FRAME = 'shell';

/** The files of the first wave that the entry imports dynamically, by built file name. */
export function firstWaveFiles(bundle: Readonly<Record<string, BuiltChunk>>): string[] {
  const files: string[] = [];
  for (const item of Object.values(bundle)) {
    if (item.type !== 'chunk') continue;
    if (item.name === FRAME) files.push(item.fileName);
  }
  return files.sort();
}

export function firstWavePlugin(): Plugin {
  return {
    name: 'first-wave',
    apply: 'build',
    transformIndexHtml: {
      order: 'post',
      handler(_html, context): HtmlTagDescriptor[] {
        const bundle = context.bundle as unknown as Record<string, BuiltChunk> | undefined;
        if (bundle === undefined) return [];
        return firstWaveFiles(bundle).map((file) => ({ tag: 'link', attrs: { rel: 'modulepreload', crossorigin: true, href: `/${file}` }, injectTo: 'head' }));
      },
    },
  };
}
