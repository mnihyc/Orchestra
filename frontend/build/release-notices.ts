/** Record the actual production bundle inputs for release notice collection. */
import type { Plugin } from 'vite';

/** Map a transformed module or asset name back to its locked npm package. */
export function npmSource(source: string): { path: string; source: string } | undefined {
  const clean = source.replaceAll('\\', '/').split('?')[0] ?? '';
  const start = clean.indexOf('node_modules/');
  if (start < 0) return undefined;
  const relative = clean.slice(start);
  const matches = [...relative.matchAll(/node_modules\/(?:@[^/]+\/)?[^/]+/g)];
  const last = matches.at(-1);
  if (!last) return undefined;
  const end = last.index + last[0].length;
  return { path: relative.slice(0, end), source: relative.slice(end + 1) };
}

/** Record only npm package locations contributing code or assets. */
export function releaseNoticesPlugin(): Plugin {
  return {
    name: 'release-notices',
    apply: 'build',
    generateBundle(_options, bundle) {
      const packages = new Set<string>();
      for (const entry of Object.values(bundle)) {
        const sources = entry.type === 'chunk'
          ? Object.entries(entry.modules).filter(([, detail]) => detail.renderedLength !== 0).map(([path]) => path)
          : entry.originalFileNames;
        for (const source of sources) {
          const input = npmSource(source);
          if (input) packages.add(input.path);
        }
      }
      this.emitFile({ type: 'asset', fileName: '.vite/notice-packages.json', source: JSON.stringify([...packages].sort()) });
    },
  };
}
