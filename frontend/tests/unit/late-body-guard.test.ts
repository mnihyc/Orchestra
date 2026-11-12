/** A browser case reads a response's body the moment the response arrives, or takes it from the fake API's record: never from a response it awaited earlier. */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { describe, expect, it } from 'vitest';

const root = join(import.meta.dirname, '../..');
const folders = ['tests/e2e', 'tests/accessibility', 'tests/performance', 'e2e'];

function sources(folder: string): string[] {
  return readdirSync(join(root, folder)).flatMap((name) => {
    const path = join(root, folder, name);
    if (statSync(path).isDirectory()) return sources(relative(root, path));
    return name.endsWith('.ts') ? [path] : [];
  });
}

/**
 * The reads of a body from a variable that holds a response taken by `page.waitForResponse` without `arrived(...)` or `.then(...)`.
 * A name is judged by what it was last given before the read, so the same name for a response of `route.fetch()` elsewhere is no match.
 */
export function lateBodyReads(text: string): { line: number; variable: string }[] {
  const events: { at: number; kind: 'give' | 'read'; name: string; late?: boolean; from?: string }[] = [];
  const bare = (value: string) => value.includes('page.waitForResponse(') && !/\barrived\(\s*page\.waitForResponse\(/.test(value) && !value.includes('.then(');
  // A helper that hands back the wait itself.
  const helpers = new Set<string>();
  for (const found of text.matchAll(/(?:const|let)\s+(\w+)\s*=\s*(?:async\s*)?\([^)]*\)\s*=>\s*([^;]*?);/gs)) if (bare(found[2] ?? '')) helpers.add(found[1] ?? '');
  for (const found of text.matchAll(/function\s+(\w+)\s*\([^)]*\)[^{]*\{(.*?)\n\s*\}/gs)) if (bare(found[2] ?? '')) helpers.add(found[1] ?? '');
  for (const found of text.matchAll(/(?:const|let)\s+(\w+)\s*(?::[^=]+)?=\s*(?:await\s+)?([^;]*?)(?:;|$)/gs)) {
    const value = found[2] ?? '', alias = /^(\w+)$/.exec(value.trim())?.[1];
    const calls = /^(\w+)\(/.exec(value.trim())?.[1];
    const late = bare(value) || (calls !== undefined && helpers.has(calls));
    events.push({ at: found.index, kind: 'give', name: found[1] ?? '', late, ...(alias === undefined ? {} : { from: alias }) });
  }
  // A parameter of a function or a callback takes the name for itself.
  for (const found of text.matchAll(/(?:\(([\w\s,]*)\)|\b(\w+))\s*=>/g)) {
    for (const name of (found[1] ?? found[2] ?? '').split(',')) if (name.trim() !== '') events.push({ at: found.index, kind: 'give', name: name.trim(), late: false });
  }
  for (const read of text.matchAll(/\.(?:json|text|body)\(\)/g)) {
    const before = text.slice(Math.max(0, read.index - 60), read.index);
    const name = /\(\s*await\s+(\w+)\s*\)\s*$/.exec(before)?.[1] ?? /(\w+)\s*$/.exec(before)?.[1] ?? '';
    events.push({ at: read.index, kind: 'read', name });
  }
  events.sort((one, other) => one.at - other.at);
  const late = new Map<string, boolean>();
  const reads: { line: number; variable: string }[] = [];
  for (const event of events) {
    if (event.kind === 'give') late.set(event.name, event.late === true || (event.from !== undefined && late.get(event.from) === true));
    else if (late.get(event.name) === true) reads.push({ line: text.slice(0, event.at).split('\n').length, variable: event.name });
  }
  return reads;
}

describe('the bodies of responses the browser cases wait for', () => {
  it('are read as the responses arrive, through arrived(...) or replyTo(...) of the shared support, or taken from the fake API', () => {
    const late = folders.flatMap((folder) => sources(folder).flatMap((path) => lateBodyReads(readFileSync(path, 'utf8')).map((read) => `${relative(root, path)}:${read.line} ${read.variable}`)));
    expect(late).toEqual([]);
  });

  it('knows the old way when it sees it', () => {
    expect(lateBodyReads("const saved = page.waitForResponse((r) => r.ok());\nawait click();\nconst body = await (await saved).json();")).toEqual([{ line: 3, variable: 'saved' }]);
    expect(lateBodyReads("const saved = arrived(page.waitForResponse((r) => r.ok()));\nconst body = await (await saved).json();")).toEqual([]);
    expect(lateBodyReads("const saved = page.waitForResponse((r) => r.ok()).then((r) => r.json());\nconst body = await saved;")).toEqual([]);
  });
});
