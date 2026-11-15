/** A feature arrives with its own words; a failed transfer can be tried again by its local boundary. */
import i18next from 'i18next';
import { loaders, moduleParts } from 'virtual:orch-vocabulary';

let releaseReady: () => void = () => undefined;
let ready = new Promise<void>((resolve) => { releaseReady = resolve; });
let started = false, epoch = 0;
const held = new Map<string, Promise<void>>();
const full = new Map<string, unknown>();

export function beginWords(): void {
  if (started) ready = new Promise<void>((resolve) => { releaseReady = resolve; });
  started = true; epoch += 1; held.clear(); full.clear();
}
export function wordsReady(): void { releaseReady(); }
export function fullWordsAdded(locale: string): void { full.set(locale, i18next.getResourceBundle(locale, 'translation')); }
export function hasFullWords(locale: string): boolean { return full.has(locale) && full.get(locale) === i18next.getResourceBundle(locale, 'translation'); }

export async function prepareWords(part: string, language?: string): Promise<void> {
  await ready;
  const locale = (language ?? i18next.language) === 'zh-CN' ? 'zh-CN' : 'en';
  if (hasFullWords(locale)) return;
  const key = `${locale}/${part}`;
  const load = loaders[key];
  if (load === undefined) return;
  let waiting = held.get(key);
  if (waiting === undefined) {
    const version = epoch;
    waiting = load().then((words) => { if (version === epoch) i18next.addResourceBundle(locale, 'translation', words, true, true); })
      .catch((error: unknown) => { held.delete(key); throw error; });
    held.set(key, waiting);
  }
  await waiting;
}

export async function prepareModuleWords(module: string): Promise<void> {
  const part = moduleParts[module];
  if (part !== undefined) await prepareWords(part);
}
