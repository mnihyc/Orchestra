/** Names in the page the files that its first script would only find once it had run, so that they are fetched with it.
 *
 * The page's entry file is fetched with the files it imports at once (the build names those in the page itself). Two more are
 * needed at once, but are imported by what that entry runs: the workbench's frame, which every signed-in screen stands in, and
 * the English words. Left to be found, they are fetched a whole round trip after the entry has arrived; named here, they
 * travel beside it.
 */
import type { HtmlTagDescriptor, Plugin } from 'vite';

/** The members of a built chunk that this reads. */
export interface BuiltChunk {
  type: string;
  fileName: string;
  name?: string;
  facadeModuleId?: string | null;
  moduleIds?: readonly string[];
}

const WORDS = '/src/i18n/en.json';
/** The chunk the build names `shell`: the workbench's frame and what only it needs (see `vite.config.ts`). */
const FRAME = 'shell';

/** The files of the first wave that the entry imports dynamically, by built file name. */
export function firstWaveFiles(bundle: Readonly<Record<string, BuiltChunk>>): string[] {
  const files: string[] = [];
  for (const item of Object.values(bundle)) {
    if (item.type !== 'chunk') continue;
    const words = item.facadeModuleId?.replaceAll('\\', '/').endsWith(WORDS) === true;
    if (item.name === FRAME || words) files.push(item.fileName);
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
