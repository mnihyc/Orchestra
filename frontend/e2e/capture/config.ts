/** Private inputs for a review of an already running world. Nothing here starts or seeds that world. */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

export type CaptureLocale = 'en' | 'zh-CN';
export type CaptureTheme = 'light' | 'dark';
export type Person = 'ada' | 'vera' | 'ben';
export type Actor = 'ada' | 'ada-admin' | 'vera' | 'ben' | 'signed-out';
export interface Credentials { login: string; password: string }
export interface ClickTarget {
  role: 'button' | 'link' | 'menuitem' | 'tab' | 'option';
  name: string;
  /** An accessible-name prefix, for a heading that includes the current place's name. */
  startsWith?: boolean;
  /** Scope an otherwise repeated control, for example to [data-thread-header]. */
  within?: string;
}
export interface DialogState {
  id: string;
  label: string;
  url: string;
  clicks: ClickTarget[];
  dialog: string;
}
export interface UrlState { id: string; label: string; url: string; dialog?: string }
export interface CaptureConfig {
  baseURL: string;
  output: string;
  candidate: string;
  people: Record<Person, Credentials>;
  /** Copy a real screen's path once; all of its inventory tabs/sections reuse it. */
  targets: Record<string, string>;
  /** Seed path segments -> the corresponding segments in this retained real world. */
  replacements: Record<string, string>;
  /** Exact inventory URL -> a different local URL, for exceptional fixtures. */
  routes: Record<string, string>;
  /** Additional read-only opening sequences. Never confirm an act in this list. */
  dialogs: DialogState[];
  states: UrlState[];
  locales: CaptureLocale[];
}

export const VIEWPORTS = [{ width: 1440, height: 900 }, { width: 400, height: 860 }] as const;
export const THEMES: readonly CaptureTheme[] = ['light', 'dark'];
export const ACTORS: readonly Exclude<Actor, 'signed-out'>[] = ['ada', 'ada-admin', 'vera', 'ben'];
export const IDLE_MS = 30_000;
// A normal Playwright navigation bound; slow/refused content still leaves evidence instead of hanging the walk.
export const READY_MS = 30_000;
// An opener gets ten times the owner's one-second write budget before it is recorded as unavailable.
export const ACTION_MS = 10_000;
// Playwright's network-idle definition: no outstanding ordinary request for 500 ms.
export const QUIET_MS = 500;

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function object(value: unknown, name: string): Record<string, unknown> {
  if (!record(value)) throw new Error(`${name} must be an object.`);
  return value;
}
function text(value: unknown, name: string): string {
  if (typeof value !== 'string' || value.trim() === '') throw new Error(`${name} must be a nonempty string.`);
  return value;
}
function strings(value: unknown, name: string): Record<string, string> {
  return Object.fromEntries(Object.entries(object(value ?? {}, name)).map(([key, value]) => [key, text(value, name)]));
}
function localPath(value: string): string {
  if (!value.startsWith('/') || value.startsWith('//') || value.includes('\\') || /[\r\n]/.test(value)) throw new Error('Capture destinations must be paths inside the given world.');
  const parsed = new URL(value, 'http://capture.invalid');
  if (parsed.origin !== 'http://capture.invalid') throw new Error('A capture destination left the given world.');
  return value;
}
export function parseClick(value: unknown): ClickTarget {
  const row = object(value, 'click');
  const role = row.role;
  if (role !== 'button' && role !== 'link' && role !== 'menuitem' && role !== 'tab' && role !== 'option') throw new Error('A click needs a button, link, menuitem, tab or option role.');
  return { role, name: text(row.name, 'click.name'), ...(row.startsWith === true ? { startsWith: true } : {}),
    ...(row.within === undefined ? {} : { within: text(row.within, 'click.within') }) };
}

/** Validation runs before any browser or sign-in, and never prints a credential or input document. */
export function parseConfig(input: unknown, env: NodeJS.ProcessEnv = process.env): CaptureConfig {
  const row = object(input, 'capture configuration');
  let url: URL;
  try { url = new URL(text(env.ORCH_CAPTURE_URL ?? row.baseURL, 'baseURL / ORCH_CAPTURE_URL')); }
  catch { throw new Error('baseURL / ORCH_CAPTURE_URL must name a valid HTTP(S) origin.'); }
  if (!['http:', 'https:'].includes(url.protocol) || url.username !== '' || url.password !== '' || url.search !== '' || url.hash !== '' || url.pathname !== '/') throw new Error('The running world URL must be an HTTP(S) origin, without a password, query, or path.');
  const people = object(row.people, 'people');
  const readPerson = (person: Person): Credentials => {
    const entry = object(people[person], `people.${person}`);
    const name = text(entry.passwordEnv, `people.${person}.passwordEnv`);
    return { login: text(entry.login ?? person, `people.${person}.login`), password: text(env[name], `the environment variable named by people.${person}.passwordEnv`) };
  };
  const replacements = strings(row.replacements, 'replacements');
  for (const [before, after] of Object.entries(replacements)) {
    if (before === '' || /[/?#\\]/.test(before + after)) throw new Error('A replacement must name one complete path segment. Use routes for whole URLs.');
  }
  const routes = strings(row.routes, 'routes');
  for (const [before, after] of Object.entries(routes)) { localPath(before); localPath(after); }
  const targets = strings(row.targets, 'targets');
  for (const path of Object.values(targets)) {
    localPath(path);
    if (path.includes('?') || path.includes('#')) throw new Error('A target is a screen path; use routes for query-specific overrides.');
  }
  const locales: CaptureLocale[] = row.locales === undefined ? ['en'] : Array.isArray(row.locales)
    ? row.locales.map((locale: unknown) => { if (locale !== 'en' && locale !== 'zh-CN') throw new Error('locales must contain en or zh-CN.'); return locale; })
    : (() => { throw new Error('locales must be an array.'); })();
  if (locales.length === 0 || new Set(locales).size !== locales.length) throw new Error('Choose at least one locale, without duplicates.');
  if (row.dialogs !== undefined && !Array.isArray(row.dialogs)) throw new Error('dialogs must be an array.');
  const dialogs = (Array.isArray(row.dialogs) ? row.dialogs : []).map((value: unknown): DialogState => {
    const state = object(value, 'dialog state');
    if (!Array.isArray(state.clicks) || state.clicks.length === 0) throw new Error('A dialog state needs an explicit opening click list.');
    return { id: text(state.id, 'dialog.id'), label: text(state.label, 'dialog.label'), url: localPath(text(state.url, 'dialog.url')),
      clicks: state.clicks.map((click: unknown) => parseClick(click)), dialog: text(state.dialog, 'dialog.dialog') };
  });
  if (row.states !== undefined && !Array.isArray(row.states)) throw new Error('states must be an array.');
  const states = (Array.isArray(row.states) ? row.states : []).map((value: unknown): UrlState => {
    const state = object(value, 'URL state');
    return { id: text(state.id, 'state.id'), label: text(state.label, 'state.label'), url: localPath(text(state.url, 'state.url')),
      ...(state.dialog === undefined ? {} : { dialog: text(state.dialog, 'state.dialog') }) };
  });
  return { baseURL: url.origin, output: resolve(text(env.ORCH_CAPTURE_OUTPUT ?? row.output ?? '.tmp/captures', 'output')),
    candidate: text(row.candidate ?? 'running-world', 'candidate'), people: { ada: readPerson('ada'), vera: readPerson('vera'), ben: readPerson('ben') },
    targets, replacements, routes, dialogs, states, locales };
}

export function readConfig(): CaptureConfig {
  const path = process.env.ORCH_CAPTURE_CONFIG;
  if (path === undefined || path === '') throw new Error('Set ORCH_CAPTURE_CONFIG to the private review input JSON; see e2e/capture/README.md.');
  let input: unknown;
  try { input = JSON.parse(readFileSync(path, 'utf8')) as unknown; }
  catch { throw new Error('The capture input file could not be read as JSON.'); }
  return parseConfig(input);
}

/** Exact segments prevent acme -> example from corrupting acme.research or an unrelated word. */
export function mapRoute(path: string, config: Pick<CaptureConfig, 'routes' | 'replacements'>): string {
  const override = Object.hasOwn(config.routes, path) ? config.routes[path] : undefined;
  if (override !== undefined) return localPath(override);
  const url = new URL(localPath(path), 'http://capture.invalid');
  url.pathname = url.pathname.split('/').map((part) => {
    const original = decodeURIComponent(part);
    return encodeURIComponent((Object.hasOwn(config.replacements, original) ? config.replacements[original] : undefined) ?? original);
  }).join('/');
  // Scope links contain node:<id> or provider:<id>; item selections contain comma-separated ids.
  const search = new URLSearchParams();
  url.searchParams.forEach((value, key) => search.append(key, value.split(/([,:])/).map((part) => (Object.hasOwn(config.replacements, part) ? config.replacements[part] : undefined) ?? part).join('')));
  url.search = search.toString();
  return url.pathname + url.search + url.hash;
}
