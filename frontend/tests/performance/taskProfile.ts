/** On a failed timing case's retry, attribute Chromium CPU samples to the built source on the runner. */
import type { CDPSession, Page } from '@playwright/test';
import { existsSync, readFileSync } from 'node:fs';
import { SourceMap, type SourceMapPayload } from 'node:module';
import { join } from 'node:path';

const maps = new Map<string, SourceMap>();
function location(frame: { url: string; lineNumber: number; columnNumber: number; functionName: string }): string {
  const file = /^https?:\/\/[^/]+\/(assets\/[\w.$-]+\.js)$/.exec(frame.url)?.[1];
  if (file === undefined) return frame.functionName || '(anonymous)';
  let map = maps.get(file);
  if (map === undefined) {
    const path = join(process.cwd(), 'dist', file + '.map');
    if (!existsSync(path)) return `${file}:${frame.lineNumber + 1}:${frame.columnNumber + 1}`;
    map = new SourceMap(JSON.parse(readFileSync(path, 'utf8')) as SourceMapPayload);
    maps.set(file, map);
  }
  const original = map.findOrigin(frame.lineNumber + 1, frame.columnNumber + 1);
  return 'fileName' in original ? `${original.fileName}:${original.lineNumber} ${original.name ?? frame.functionName}` : file;
}

export async function startTaskProfile(page: Page, enabled: boolean): Promise<CDPSession | undefined> {
  if (!enabled) return undefined;
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Profiler.enable');
  await cdp.send('Profiler.start');
  return cdp;
}

export async function reportTaskProfile(cdp: CDPSession | undefined, phase: string): Promise<void> {
  if (cdp === undefined) return;
  const { profile } = await cdp.send('Profiler.stop');
  await cdp.detach();
  const nodes = new Map(profile.nodes.map((node) => [node.id, node]));
  const parents = new Map<number, number>();
  for (const node of profile.nodes) for (const child of node.children ?? []) parents.set(child, node.id);
  const totals = new Map<number, number>();
  for (const [index, id] of (profile.samples ?? []).entries()) {
    totals.set(id, (totals.get(id) ?? 0) + (profile.timeDeltas?.[index] ?? 0));
  }
  const hot = [...totals].sort((a, b) => b[1] - a[1]).slice(0, 15).map(([id, micros]) => {
    const stack: string[] = [];
    for (let at: number | undefined = id; at !== undefined && stack.length < 8; at = parents.get(at)) {
      const node = nodes.get(at);
      if (node === undefined) break;
      stack.push(location(node.callFrame));
    }
    return { ms: Math.round(micros / 1000), stack };
  });
  process.stdout.write(`CPU samples for ${phase} ${JSON.stringify(hot)}\n`);
}
