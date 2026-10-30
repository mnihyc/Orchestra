/** The groups the finder answers from what the page already holds: commands, settings, the rows of permission pages and help.
 *
 * None of these asks the server for each key; each is a list held in the page, made ready to be matched
 * once, and ranked again for every letter typed. A group that has nothing to say of the letters typed
 * is empty and is not drawn.
 */
import { screenBuilt } from '@/shell/availability';
import { actionKeys } from '@/shell/keys';
import { articleAvailable } from '@/help/guide';
import type { TFunction } from 'i18next';
import { BookOpen, CircleQuestionMark, Cog, Keyboard, ListChecks, Lock, SquareTerminal } from 'lucide-react';

import type { Command } from '@/features/finder/commands';
import type { Destination } from '@/features/finder/destinations';
import type { Env, FinderRow, Scored } from '@/features/finder/rows';
import { markedRanges, type Prepared, prepare, rank, type Searchable } from '@/features/finder/ranking';
import { numericSettingDisplay, numericSettingText } from '@/features/settings/settingText';
import type { PermissionDef, ResolvedSetting, SettingDef, TreeNode } from '@/generated/api-types';
import { i18n } from '@/i18n';
import type { Article } from '@/help/guide';

/** An item made ready to be matched. */
export interface Entry<Item> {
  item: Item;
  prepared: Prepared;
}

export function entriesOf<Item>(items: readonly Item[], searchable: (item: Item) => Searchable): Entry<Item>[] {
  return items.map((item) => ({ item, prepared: prepare(searchable(item)) }));
}

/** The entries that answer the query, best first. With nothing typed, all of them in the order they were given. */
export function answer<Item>(entries: readonly Entry<Item>[], query: string): { item: Item; score: number }[] {
  return rank(entries, (entry) => entry.prepared, query).map(({ entry, score }) => ({ item: entry.item, score }));
}

// ---------------------------------------------------------------- commands

/** In what order the kinds of command are listed when nothing is typed: what can be done to the place in view first. */
const KIND_ORDER: Record<Command['kind'], number> = { place: 0, create: 1, go: 2, view: 3, appearance: 4, account: 5 };

/** The commands made ready, in the order of their kinds and then of their registration. */
export function commandEntries(commands: readonly Command[], t: TFunction): Entry<Command>[] {
  const ordered = commands.map((command, index) => ({ command, index })).sort((one, other) => KIND_ORDER[one.command.kind] - KIND_ORDER[other.command.kind] || one.index - other.index);
  return entriesOf(
    ordered.map(({ command }) => command),
    (command) => ({ title: command.title(t), words: command.keywords?.(t) ?? [], detail: command.about?.(t) ?? t(`finder.commandKinds.${command.kind}`) }),
  );
}

export function commandRows(entries: readonly Entry<Command>[], query: string, env: Env): Scored[] {
  const rows: Scored[] = [];
  for (const { item: command, score } of answer(entries, query)) {
    const availability = command.available?.() ?? true;
    if (availability === false) continue;
    const title = command.title(env.t);
    const disabled = availability === true ? undefined : availability.disabled;
    const row: FinderRow = {
      id: `commands:${command.id}`,
      group: 'commands',
      ...(command.keeps === undefined ? {} : { keeps: command.keeps }),
      title,
      marks: markedRanges(title, query),
      detail: disabled ?? command.about?.(env.t) ?? env.t(`finder.commandKinds.${command.kind}`),
      ...(command.note === undefined ? {} : { note: command.note(env.t) }),
      icon: command.icon ?? SquareTerminal,
      ...(disabled === undefined ? {} : { disabled }),
      ...(command.shortcut === undefined ? {} : { shortcut: command.shortcut }),
      ...(command.path === undefined ? {} : { destination: { to: command.path } }),
      run: (how) => {
        // A refusal or permission change after the row was drawn wins over its old enabled state.
        if ((command.available?.() ?? true) !== true) return;
        if (command.path === undefined) command.run(how);
        else env.go({ to: command.path }, how);
      },
    };
    rows.push({ row, score });
  }
  return rows;
}

// ---------------------------------------------------------------- settings

/** The areas the keys of the registry begin with, in words; a prefix that is not here is written as it is, with a capital. */
const AREAS = {
  access: 'finder.areas.access',
  agent: 'finder.areas.agent',
  approval: 'finder.areas.approval',
  auth: 'finder.areas.auth',
  auto: 'finder.areas.auto',
  binding: 'finder.areas.binding',
  broadcast: 'finder.areas.broadcast',
  capability: 'finder.areas.capability',
  chat: 'finder.areas.chat',
  collab: 'finder.areas.collab',
  docs: 'finder.areas.docs',
  env: 'finder.areas.env',
  exec: 'finder.areas.exec',
  hosts: 'finder.areas.hosts',
  invitations: 'finder.areas.invitations',
  models: 'finder.areas.models',
  orch_tools: 'finder.areas.orch_tools',
  providers: 'finder.areas.providers',
  registration: 'finder.areas.registration',
  retention: 'finder.areas.retention',
  runtime: 'finder.areas.runtime',
  sandbox: 'finder.areas.sandbox',
  sharing: 'finder.areas.sharing',
  storage: 'finder.areas.storage',
  switch: 'finder.areas.switch',
  terminal: 'finder.areas.terminal',
  thread: 'finder.areas.thread',
  usage: 'finder.areas.usage',
  workspace: 'finder.areas.workspace',
} as const;

/** Words for the area a key belongs to: the part of its name before the first dot. */
export function areaOf(key: string, t: TFunction): string {
  const prefix = key.split('.')[0] ?? key;
  const known = (AREAS as Record<string, (typeof AREAS)[keyof typeof AREAS] | undefined>)[prefix];
  return known === undefined ? prefix.replace(/_/g, ' ').replace(/^./, (letter) => letter.toUpperCase()) : t(known);
}

/** A number of minutes as a person says it: "30 min", "1 day". */
export function minutesAsWords(minutes: number, t: TFunction): string {
  if (minutes === 0) return t('finder.value.never');
  if (minutes % 1_440 === 0) return t('finder.value.days', { count: minutes / 1_440 });
  if (minutes % 60 === 0) return t('finder.value.hours', { count: minutes / 60 });
  return t('finder.value.minutes', { count: minutes });
}

/** A value as the row of a setting says it, as far as it can be said in a few words; nothing for what cannot. */
export function valueAsWords(def: Pick<SettingDef, 'schema' | 'display'>, value: unknown, t: TFunction): string | null {
  const schema = typeof def.schema === 'object' && def.schema !== null ? (def.schema as Record<string, unknown>) : {};
  if (typeof value === 'boolean') return t(value ? 'finder.value.on' : 'finder.value.off');
  if (typeof value === 'number') {
    // What the registry declares a number to be says it: a size as a size, and a 0 that means never as Never.
    const display = numericSettingDisplay(def);
    if ((def.display !== undefined && def.display !== null || Array.isArray(schema.anyOf)) && display !== null) return numericSettingText(value, display, i18n.resolvedLanguage ?? 'en', t);
    switch (schema['x-unit']) {
      case 'minutes':
        return minutesAsWords(value, t);
      case 'hours':
        return t('finder.value.hours', { count: value });
      case 'days':
        return t('finder.value.days', { count: value });
      case 'seconds':
        return t('finder.value.seconds', { count: value });
      default:
        return String(value);
    }
  }
  if (typeof value === 'string' && value !== '' && value.length <= 32) return value;
  if (Array.isArray(value)) return t('finder.value.entries', { count: value.length });
  return null;
}

/** What a row of a setting says of where it is set at the place in view: "set for Web (1 day)", "Acme decides this". */
export function whereSet(resolved: ResolvedSetting | undefined, def: SettingDef, t: TFunction): string | null {
  if (resolved === undefined) return null;
  const value = valueAsWords(def, resolved.value, t);
  const decider = resolved.locked_by?.node?.name ?? null;
  if (decider !== null) return t('finder.setting.decides', { name: decider });
  const node = resolved.source.node;
  if (resolved.source.scope === 'default' || (node?.name ?? null) === null) return value === null ? t('finder.setting.default') : t('finder.setting.defaultValue', { value });
  return value === null ? t('finder.setting.setFor', { name: node?.name ?? '' }) : t('finder.setting.setForValue', { name: node?.name ?? '', value });
}

export function settingEntries(defs: readonly SettingDef[]): Entry<SettingDef>[] {
  return entriesOf(defs, (def) => ({
    title: def.title,
    // The key is what a person who knows it types; its parts find it too, and so do the words the catalogue gives it.
    words: [def.key, def.key.replace(/[._]/g, ' '), ...def.keywords],
    detail: def.description,
  }));
}

/** The nearest place of the screen in view where a setting can be set: the place itself, or the nearest one above it that the key allows. */
export function nearestPlace(def: Pick<SettingDef, 'scopes'>, path: readonly Pick<TreeNode, 'id' | 'kind'>[]): { id: string; kind: TreeNode['kind'] } | null {
  for (let at = path.length - 1; at >= 0; at -= 1) {
    const place = path[at];
    // A thread has no settings of its own to read in a screen: its topic's are the nearest.
    if (place !== undefined && place.kind !== 'thread' && def.scopes.includes(place.kind)) return { id: place.id, kind: place.kind };
  }
  return null;
}

/** Where a setting opens: the settings of the nearest place, with the key in the address so that the row can be lit. */
export function settingDestination(def: SettingDef, place: { id: string; kind: TreeNode['kind'] } | null, rootSlug: (orgId: string) => string): Destination {
  const search = { setting: def.key };
  if (place === null) return def.scopes.includes('system') ? { to: '/admin/$section', params: { section: 'settings' }, search } : { to: '/settings/profile', search };
  switch (place.kind) {
    case 'org':
      return { to: '/org/$slug/settings/$section', params: { slug: rootSlug(place.id), section: 'policies' }, search };
    case 'project':
      return { to: '/project/$projectId/settings', params: { projectId: place.id }, search: {section:'policies',...search} };
    case 'topic':
    case 'thread':
      return { to: '/topic/$topicId', params: { topicId: place.id }, search: { tab: 'settings', section:'policies', ...search } };
  }
}

export function settingRows(
  entries: readonly Entry<SettingDef>[],
  query: string,
  env: Env,
  at: { resolved: ReadonlyMap<string, ResolvedSetting>; place: (def: SettingDef) => Destination },
): Scored[] {
  return answer(entries, query).map(({ item: def, score }) => {
    const where = whereSet(at.resolved.get(def.key), def, env.t);
    const destination = at.place(def);
    const row: FinderRow = {
      id: `settings:${def.key}`,
      group: 'settings',
      title: def.title,
      marks: markedRanges(def.title, query),
      detail: [areaOf(def.key, env.t), where ?? env.t('finder.setting.scopes', { scopes: def.scopes.map((scope)=>env.t(`settings.editor.detail.scope.${scope}`,{defaultValue:env.t('resourceDetails.layout.notReported')})).join(env.t('startFlow.separator')) })].join(': '),
      icon: Cog,
      destination,
      run: (how) => env.go(destination, how),
    };
    return { row, score };
  }).filter(({ row }) => row.destination === undefined || screenBuilt(row.destination.to));
}

// ---------------------------------------------------------------- the rows of a permission page

export function permissionEntries(defs: readonly PermissionDef[]): Entry<PermissionDef>[] {
  return entriesOf(defs, (def) => ({ title: def.label, words: [def.name, def.name.replace(/[._]/g, ' '), def.area], detail: def.allows }));
}

export function permissionRows(entries: readonly Entry<PermissionDef>[], query: string, open: (permission: string) => void): Scored[] {
  return answer(entries, query).map(({ item: def, score }) => ({
    score,
    row: {
      id: `rows:${def.name}`,
      group: 'rows',
      title: def.label,
      marks: markedRanges(def.label, query),
      detail: def.allows,
      icon: Lock,
      run: () => open(def.name),
    },
  }));
}

// ---------------------------------------------------------------- help

/** What an entry of the help is: an article, or a heading in one. */
export interface HelpEntry {
  article: Article;
  /** The heading it leads to; absent for the article as a whole. */
  heading?: Pick<Article['headings'][number], 'id' | 'text' | 'keywords'>;
}

export function helpEntries(articles: readonly Article[]): Entry<HelpEntry>[] {
  const all: HelpEntry[] = articles.filter(articleAvailable).flatMap((article) => [{ article }, ...article.headings.map((heading) => ({ article, heading }))]);
  return entriesOf(all, (entry) =>
    entry.heading === undefined
      ? { title: entry.article.title, words: entry.article.keywords, detail: '' }
      : { title: entry.heading.text, words: [entry.article.title, ...(entry.heading.keywords ?? [])], detail: '' },
  );
}

/** The same shipped, localized help topics also act from `>`, using the existing single Help surface. */
export function helpCommands(articles: readonly Article[], open: (article: string) => void): Command[] {
  return articles.filter(articleAvailable).map((article) => ({ id: `help.article.${article.id}`, kind: 'go',
    title: (t) => t('commands.helpTopic', { topic: article.title }),
    keywords: (t) => [...t('commands.words.help').split(/\s+/), ...article.keywords], icon: BookOpen,
    run: () => open(article.id),
  }));
}

export function helpRows(entries: readonly Entry<HelpEntry>[], query: string, env: Env, open: (article: string, heading?: string) => void): Scored[] {
  return answer(entries, query).map(({ item, score }) => {
    const title = item.heading?.text ?? item.article.title;
    return {
      score,
      row: {
        id: `help:${item.article.id}:${item.heading?.id ?? ''}`,
        group: 'help',
        title,
        marks: markedRanges(title, query),
        detail: item.heading === undefined ? env.t('finder.help.article') : item.article.title,
        icon: item.heading === undefined ? BookOpen : CircleQuestionMark,
        note: env.t('finder.help.articleNote'),
        beside: true,
        destination: { to: '/help/$article', params: { article: item.article.id } },
        run: (how) => {
          if (how === 'tab') env.go({ to: '/help/$article', params: { article: item.article.id }, search: {} }, how);
          else open(item.article.id, item.heading?.id);
        },
      } satisfies FinderRow,
    };
  });
}

/** The rows that stand for the finder's own offers when the field is empty. */
export function offerRows(env: Env, helpForScreen: () => void, commandsHere: () => void): Scored[] {
  const { t } = env;
  // The help's keys are the person's own where they chose some, and none where they took them away.
  const helpKeys = actionKeys('help');
  return [
    { row: { id: 'actions:here', group: 'actions' as const, title: t('finder.offers.here'), marks: [], detail: t('finder.offers.hereAbout'), icon: ListChecks, keeps: true, run: commandsHere }, score: 0 },
    { row: { id: 'actions:help', group: 'actions' as const, title: t('finder.offers.help'), marks: [], detail: t('finder.offers.helpAbout'), icon: Keyboard, ...(helpKeys === undefined ? {} : { shortcut: helpKeys }), run: helpForScreen }, score: 0 },
  ];
}
