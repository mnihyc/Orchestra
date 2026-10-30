/** A cap in words, the same words wherever a cap is shown: whom it covers, what it allows, of which models and with which agent, at which speed, where it was set, and when it starts again.
 *
 * "Everyone together · all models · project Web" is a row of the Limits card; "Everyone together may
 * use at most 2M tokens a day on all models in project Web." is the sentence the form completes and
 * a confirmation repeats. Both are made from the same parts, so that a cap is never described two ways.
 * Times that a cap starts again are told in UTC, which is the clock periods are counted by.
 */
import type { TFunction } from 'i18next';
import { i18n } from '@/i18n';

import type { CapStatus, HarnessKindName, NodeRef, ProviderRef, UsageCap } from '@/generated/api-types';
import { decimal, formatCredits } from '@/features/usage/figures';
import { formatNumber, formatTokens } from '@/lib/format';
import { DETAIL_PAGE_ROWS } from '@/features/resources/DetailSections';
import { programName } from '@/features/resources/programNames';

/** A registry name, supplied by the view rather than guessed from a program's kind. */
export type ProgramTitle = (kind: string) => string | null;

/** A sentence previews one shared-size page; large selections never become a large text node. */
function selectionWords(values: readonly string[], label: (value: string, index: number) => string, t: TFunction, locale: string): string {
  const shown: string[] = [];
  const limit = Math.min(values.length, DETAIL_PAGE_ROWS);
  for (const [index, value] of values.slice(0, limit).entries()) shown.push(label(value, index));
  const names = shown.join(', ');
  return values.length > limit ? t('caps.selectionMore', { names, total: formatNumber(values.length - limit, locale) }) : names;
}

/** Whom a cap covers. */
export function whoWords(cap: Pick<UsageCap, 'subject' | 'split'>, t: TFunction): string {
  const each = cap.split === 'each';
  switch (cap.subject.kind) {
    case 'everyone':
      return each ? t('caps.who.each') : t('caps.who.everyone');
    case 'user':
      return cap.subject.user.display_name;
    case 'group':
      return each ? t('caps.who.eachMember', { name: `${cap.subject.group.name}, ${cap.subject.group.place ?? t('access.place.hidden')}` }) : t('caps.who.groupTogether', { name: `${cap.subject.group.name}, ${cap.subject.group.place ?? t('access.place.hidden')}` });
    case 'role':
      return cap.subject.at === null || cap.subject.at === undefined ? t('caps.who.role', { role: t(`roles.${cap.subject.role}`) }) : cap.subject.role === 'viewer' ? t('access.holder.everyoneIn', { place: cap.subject.at.name }) : t('access.holder.roleOf', { roles: t(`access.holder.roles.${cap.subject.role}`), place: cap.subject.at.name });
    case 'members':
      return t('access.holder.membersOf', { place: cap.subject.node.name });
  }
}

/** A model pattern as a reader says it: a name, a family by its beginning, or a group of a provider's models. */
export function patternWords(pattern: string, t: TFunction): string {
  if (pattern.startsWith('@')) return t('caps.on.group', { name: pattern.slice(1) });
  return pattern.endsWith('*') ? `${pattern.slice(0, -1)}…` : pattern;
}

/** The models a cap covers: every one when it names none. */
export function modelsWords(patterns: readonly string[], t: TFunction): string {
  return patterns.length === 0 ? t('caps.on.all') : selectionWords(patterns, (pattern) => patternWords(pattern, t), t, i18n.resolvedLanguage ?? i18n.language);
}

/** What a tier is called: its id, with a capital. */
export function tierWords(tier: string): string {
  return tier.length === 0 ? tier : `${tier.charAt(0).toUpperCase()}${tier.slice(1)}`;
}

/** What else a cap narrows its models to, as phrases to follow them: the agents and the speeds. */
export function narrowingWords(cap: { harness_kinds: readonly string[]; tiers: readonly string[]; harnesses?: readonly HarnessKindName[] }, t: TFunction, programTitle?: ProgramTitle, locale = i18n.resolvedLanguage ?? i18n.language): string[] {
  const hasNames = cap.harnesses !== undefined && cap.harnesses.length > 0;
  return [
    ...(cap.harness_kinds.length === 0 ? [] : [t('caps.with', { agents: !hasNames && programTitle === undefined
      ? t('caps.programNamesUnreported', { count: cap.harness_kinds.length, total: formatNumber(cap.harness_kinds.length, locale) })
      : selectionWords(cap.harness_kinds, (kind, index) => {
        const named = cap.harnesses?.[index];
        // The API names the selected kinds in the same order; only the visible prefix is read.
        return (!hasNames ? programTitle?.(kind) : named?.kind === kind ? programName(named, locale) : null) ?? t('resources.harnessNameUnknown');
      }, t, locale) })]),
    ...(cap.tiers.length === 0 ? [] : [t('caps.speed', { speeds: selectionWords(cap.tiers, tierWords, t, locale) })]),
  ];
}

/** Where a cap was set: a place, a provider, both, or the whole system. */
export function whereWords(cap: { node: Pick<NodeRef, 'kind' | 'name' | 'redacted'> | null; provider: Pick<ProviderRef, 'name'> | null }, t: TFunction): string {
  const place = cap.node === null ? null : placeWords(cap.node, t);
  if (cap.provider !== null) return place === null ? t('caps.where.provider', { name: cap.provider.name }) : t('caps.where.providerIn', { name: cap.provider.name, place });
  return place ?? t('caps.where.system');
}

/** A place, with what kind it is: "Acme", "project Web". */
export function placeWords(node: Pick<NodeRef, 'kind' | 'name' | 'redacted'>, t: TFunction): string {
  if (node.redacted || node.name === null) return t('usage.key.hidden', { kind: t(`usage.key.kind.${node.kind}`) });
  return t(`caps.where.${node.kind}`, { name: node.name });
}

/** The row names who may use which models and where, without exposing an account's address. */
export function capTitle(cap: UsageCap, t: TFunction, programTitle?: ProgramTitle): string {
  return t('caps.title',{who:whoWords(cap,t),models:[modelsWords(cap.models,t),...narrowingWords(cap,t,programTitle)].join(' '),where:whereWords(cap,t)});
}

/** The day a period begins on, written for the reader's language; the days are UTC. A day that is none (a server that has not got one to give) is written as it came. */
export function dayWords(day: string | null | undefined, locale: string): string {
  if (day === null || day === undefined) return '';
  const date = new Date(`${day.slice(0, 10)}T00:00:00Z`);
  return Number.isNaN(date.getTime()) ? day : new Intl.DateTimeFormat(locale, { timeZone: 'UTC', month: 'short', day: 'numeric', year: 'numeric' }).format(date);
}

/** The period a cap counts in: "a day", "in total, counted from 3 Oct". */
export function periodWords(cap: Pick<UsageCap, 'period' | 'counted_from'>, t: TFunction, locale: string): string {
  if (cap.period === null) return '';
  if (cap.period === 'total') return cap.counted_from === null ? t('caps.per.total') : t('caps.per.totalFrom', { date: dayWords(cap.counted_from, locale) });
  return t(`caps.per.${cap.period}`);
}

/** The amount of a cap with its unit: "2M tokens", "500 credits", "4 turns". */
export function amountFigure(cap: Pick<UsageCap, 'amount' | 'meter'>, t: TFunction, locale: string): string {
  const amount = decimal(cap.amount);
  switch (cap.meter) {
    case 'tokens':
      return `${formatTokens(amount, locale)} ${t('caps.unit.tokens')}`;
    case 'credits':
      return `${formatNumber(amount, locale, { maximumFractionDigits: 6 })} ${t('caps.unit.credits')}`;
    case 'running':
      return t('caps.unit.running', { count: amount });
  }
}

/** What a cap allows, as the row says it: "at most 2M tokens a day", "at most 4 turns at once", "no limit". */
export function allowanceWords(cap: Pick<UsageCap, 'amount' | 'meter' | 'period' | 'counted_from'>, t: TFunction, locale: string): string {
  if (cap.amount === null) return t('caps.amount.none');
  const figure = amountFigure(cap, t, locale);
  return cap.meter === 'running' ? t('caps.amount.atMost', { figure }) : t('caps.amount.atMostPer', { figure, per: periodWords(cap, t, locale) });
}

/** What is used of a cap, in its own unit: "2.14M of 2M tokens", "3 of 4". */
export function usedWords(status: Pick<CapStatus, 'used'> & { cap: Pick<UsageCap, 'amount' | 'meter'> }, t: TFunction, locale: string): string {
  const used = decimal(status.used);
  const shown = status.cap.meter === 'tokens' ? formatTokens(used, locale) : status.cap.meter === 'credits' ? formatCredits(used, locale) : formatNumber(used, locale);
  if (status.cap.amount === null) return t('caps.used.noLimit', { used: shown });
  if (status.cap.meter === 'running') return t('caps.used.running', { used: shown, amount: formatNumber(decimal(status.cap.amount), locale) });
  return t('caps.used.of', { used: shown, figure: amountFigure(status.cap, t, locale) });
}

/** What the sentence of a cap is made of, as words and as the choices themselves. */
export interface SentenceParts {
  who: string;
  /** The amount as the cap has it; none for no limit. */
  amount: string | null;
  meter: UsageCap['meter'];
  period: UsageCap['period'];
  counted_from: string | null;
  models: readonly string[];
  harness_kinds: readonly string[];
  harnesses?: UsageCap['harnesses'];
  tiers: readonly string[];
  node: Pick<NodeRef, 'kind' | 'name' | 'redacted'> | null;
  provider: Pick<ProviderRef, 'name'> | null;
  /** The amount in words, where it is not one yet (the form while it is being filled in). */
  figureText?: string | undefined;
}

/** The whole sentence a cap says: what the form completes and a confirmation repeats. */
export function sentenceOf(parts: SentenceParts, t: TFunction, locale: string, programTitle?: ProgramTitle): string {
  const words = {
    who: parts.who,
    on: modelsWords(parts.models, t),
    extra: narrowingWords(parts, t, programTitle, locale)
      .map((phrase) => ` ${phrase}`)
      .join(''),
    where: whereWords(parts, t),
    figure: parts.amount === null ? '' : (parts.figureText ?? amountFigure({ amount: parts.amount, meter: parts.meter }, t, locale)),
    per: periodWords({ period: parts.period, counted_from: parts.counted_from }, t, locale),
  };
  if (parts.amount === null) return t('caps.sentence.none', words);
  return parts.meter === 'running' ? t('caps.sentence.run', words) : t('caps.sentence.use', words);
}

/** The sentence of a cap that exists. */
export function capSentence(cap: UsageCap, t: TFunction, locale: string, programTitle?: ProgramTitle): string {
  return sentenceOf({ ...cap, who: whoWords(cap, t) }, t, locale, programTitle);
}

/** When a cap starts again, in UTC: "00:00 UTC" for a day, the weekday for a week, the day for a month. */
export function startsAgain(iso: string, period: UsageCap['period'], locale: string, t: TFunction): string {
  const at = new Date(iso);
  const format = (options: Intl.DateTimeFormatOptions) => new Intl.DateTimeFormat(locale, { timeZone: 'UTC', ...options }).format(at);
  const when =
    period === 'day'
      ? t('caps.utcTime', { time: format({ hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }) })
      : period === 'week'
        ? format({ weekday: 'long' })
        : format({ day: 'numeric', month: 'long' });
  return t('caps.startsAgain', { when });
}

/** Whether a provider is the one a cap is defined on. */
export function onProvider(cap: Pick<UsageCap, 'provider'>, provider: Pick<ProviderRef, 'id'>): boolean {
  return cap.provider?.id === provider.id;
}
