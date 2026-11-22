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

interface TaskProfile { cdp: CDPSession; before: Map<string, number> }
export interface TaskWindow { startTime: number; duration: number; label?: string }

export async function startTaskProfile(page: Page, enabled: boolean): Promise<TaskProfile | undefined> {
  if (!enabled) return undefined;
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Performance.enable');
  const { metrics } = await cdp.send('Performance.getMetrics');
  await cdp.send('Profiler.enable');
  await cdp.send('Profiler.start');
  return { cdp, before: new Map(metrics.map(({ name, value }) => [name, value])) };
}

export async function reportTaskProfile(recording: TaskProfile | undefined, phase: string, windows: readonly TaskWindow[]): Promise<void> {
  if (recording === undefined) return;
  const { cdp, before } = recording;
  const { profile } = await cdp.send('Profiler.stop');
  const { metrics } = await cdp.send('Performance.getMetrics');
  await cdp.detach();
  // Chromium accounts for style and layout outside the JavaScript sampled stacks.
  const phases = new Set(['TaskDuration', 'ScriptDuration', 'LayoutDuration', 'RecalcStyleDuration']);
  const times = metrics.flatMap(({ name, value }) => {
    const start = before.get(name);
    return phases.has(name) && start !== undefined ? [{ name, ms: Math.round((value - start) * 1000) }] : [];
  });
  process.stdout.write(`Renderer time for ${phase} ${JSON.stringify(times)}\n`);
  const nodes = new Map(profile.nodes.map((node) => [node.id, node]));
  const parents = new Map<number, number>();
  for (const node of profile.nodes) for (const child of node.children ?? []) parents.set(child, node.id);
  const totals = new Map<number, number>();
  // CDP uses monotonic seconds for NavigationStart and microseconds for its CPU profile.
  // DOM performance entries are milliseconds from that same navigation.
  const navigation = before.get('NavigationStart');
  let stamp = profile.startTime;
  const intervals = windows.map(({ startTime, duration, label }) => {
    const start = (navigation ?? 0) * 1_000_000 + startTime * 1_000;
    return { start, end: start + duration * 1_000, label, totals: new Map<number, number>() };
  });
  for (const [index, id] of (profile.samples ?? []).entries()) {
    const delta = profile.timeDeltas?.[index] ?? 0;
    const end = stamp + delta;
    totals.set(id, (totals.get(id) ?? 0) + delta);
    if (navigation !== undefined) for (const interval of intervals) {
      const overlap = Math.min(end, interval.end) - Math.max(stamp, interval.start);
      if (overlap > 0) interval.totals.set(id, (interval.totals.get(id) ?? 0) + overlap);
    }
    stamp = end;
  }
  const hottest = (times: ReadonlyMap<number, number>) => [...times].sort((a, b) => b[1] - a[1]).slice(0, 15).map(([id, micros]) => {
    const stack: string[] = [];
    for (let at: number | undefined = id; at !== undefined && stack.length < 8; at = parents.get(at)) {
      const node = nodes.get(at);
      if (node === undefined) break;
      stack.push(location(node.callFrame));
    }
    return { ms: Math.round(micros / 1000), stack };
  });
  process.stdout.write(`CPU samples for ${phase} ${JSON.stringify(hottest(totals))}\n`);
  // A shared component may have many small sampled stacks. Count each sample once
  // at its nearest application source and keep framework-only work visible too.
  const sources = new Map<number, string | null>();
  function nearestSource(id: number): string | null {
    const held = sources.get(id);
    if (held !== undefined) return held;
    const node = nodes.get(id);
    if (node === undefined) return null;
    const frame = location(node.callFrame);
    const file = frame.split(/:\d+ /, 1)[0] ?? frame;
    const parent = parents.get(id);
    const source = file.startsWith('../../src/') ? file.slice(6) : parent === undefined ? null : nearestSource(parent);
    sources.set(id, source);
    return source;
  }
  if (navigation !== undefined) for (const interval of intervals) {
    process.stdout.write(`CPU samples inside ${interval.label ?? phase} ${Math.round((interval.end - interval.start) / 1000)} ms ${JSON.stringify(hottest(interval.totals))}\n`);
    const bySource = new Map<string, number>();
    for (const [id, micros] of interval.totals) {
      const node = nodes.get(id);
      const source = nearestSource(id);
      const owner = source ?? (node === undefined ? '(unknown)' : location(node.callFrame).split(/:\d+ /, 1)[0] ?? '(runtime)');
      bySource.set(owner, (bySource.get(owner) ?? 0) + micros);
    }
    const owners = [...bySource].sort((a, b) => b[1] - a[1]).slice(0, 15).map(([source, micros]) => ({ source, ms: Math.round(micros / 1000) }));
    process.stdout.write(`Sampled time by source inside ${interval.label ?? phase} ${JSON.stringify(owners)}\n`);
  }
}
