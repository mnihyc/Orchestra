/** What a cap that is being made or changed holds while it is edited, the rules it must keep before it is sent, and what it becomes on the wire.
 *
 * Pure functions, so that every rule the form shows can be tested without drawing it. The rules are the
 * API's own, said before they are asked: a cap for one person is that person's alone, a cap for a role is
 * counted for each person, an amount is whole where the meter counts whole things and has at most six
 * decimals in credits, a cap on turns at once has no period, warning or answer to an uncounted turn, and a
 * warning is a percent from one to ninety-nine.
 */
import type { TFunction } from 'i18next';

import type { CapAtLimit, CapInput, CapMeter, CapPeriod, CapSplit, CapSubjectInput, CapUncounted, Role, UsageCap, UsageCapUpdateBody } from '@/generated/api-types';

/** Whom the cap is for, as the form offers it: six ways, each with its own count of people. */
export type Who = 'everyone' | 'each' | 'user' | 'group' | 'eachGroup' | 'role' | 'members' | 'eachMembers';

export const WHO: readonly Who[] = ['everyone', 'each', 'user', 'group', 'eachGroup', 'role', 'members', 'eachMembers'];

/** The warning the form starts with, in percent of the amount. */
export const DEFAULT_WARN_PERCENT = 80;

/** The least and the most a warning may be, in percent: the contract's own. */
export const WARN_PERCENT = { least: 1, most: 99 } as const;

/** How many decimals an amount of credits has at most: the contract's own. */
export const CREDIT_DECIMALS = 6;

/** What a cap holds while it is edited: texts for what is typed, and the choices as they are. */
export interface CapDraft {
  who: Who;
  /** The person, for `user`. */
  userId: string;
  /** The group, for `group` and `eachGroup`. */
  groupId: string;
  /** The role, for `role`. */
  role: Role;
  /** The selected place whose role is named. */
  roleAt: string;
  /** The selected org whose membership by domain is named. */
  membersId: string;
  meter: CapMeter;
  /** What is typed: a number, with `k` or `M` for thousands and millions of tokens. */
  amount: string;
  /** No limit at all: it exempts the people named from an allowance that covers everyone. */
  noLimit: boolean;
  period: CapPeriod;
  /** A day, for the period `total`; nothing for from the beginning. */
  countedFrom: string;
  models: string[];
  harnessKinds: string[];
  tiers: string[];
  /** A percent, or nothing for no warning. */
  warn: string;
  atLimit: CapAtLimit;
  uncounted: CapUncounted;
  note: string;
}

/** A cap as the form starts: everyone together, a day's tokens, a warning at 80 %, the work that runs stopped at the cap. */
export function emptyDraft(overrides: Partial<CapDraft> = {}): CapDraft {
  return {
    who: 'everyone',
    userId: '',
    groupId: '',
    role: 'developer',
    roleAt: '',
    membersId: '',
    meter: 'tokens',
    amount: '',
    noLimit: false,
    period: 'day',
    countedFrom: '',
    models: [],
    harnessKinds: [],
    tiers: [],
    warn: String(DEFAULT_WARN_PERCENT),
    atLimit: 'stop',
    uncounted: 'allow',
    note: '',
    ...overrides,
  };
}

/** The split a way of naming people comes with: one amount for all of them together, or that amount for each. */
export function splitOf(who: Who): CapSplit {
  return who === 'each' || who === 'eachGroup' || who === 'role' || who === 'eachMembers' ? 'each' : 'shared';
}

/** The subject as the request names it. */
export function subjectOf(draft: CapDraft): CapSubjectInput {
  switch (draft.who) {
    case 'everyone':
    case 'each':
      return { kind: 'everyone' };
    case 'user':
      return { kind: 'user', id: draft.userId };
    case 'group':
    case 'eachGroup':
      return { kind: 'group', id: draft.groupId };
    case 'role':
      return { kind: 'role', role: draft.role, ...(draft.roleAt === '' ? {} : { at: draft.roleAt }) };
    case 'members': case 'eachMembers':
      return { kind: 'members', id: draft.membersId };
  }
}

/** The way of naming people that a cap's subject and split come to. */
export function whoOf(cap: Pick<UsageCap, 'subject' | 'split'>): Who {
  switch (cap.subject.kind) {
    case 'everyone':
      return cap.split === 'each' ? 'each' : 'everyone';
    case 'user':
      return 'user';
    case 'group':
      return cap.split === 'each' ? 'eachGroup' : 'group';
    case 'role':
      return 'role';
    case 'members':
      return cap.split === 'each' ? 'eachMembers' : 'members';
  }
}

// ---------------------------------------------------------------- the amount

const TOKEN_SUFFIX = { k: 1_000, m: 1_000_000 } as const;

/**
 * The amount a person typed, as the text the API takes: whole tokens or turns, credits to six decimals.
 * Tokens may be written with `k` or `M` ("500k", "1.5M"). Nothing for a text that is none.
 */
export function amountText(typed: string, meter: CapMeter): string | null {
  const text = typed.trim().replace(/[\s,_]/g, '');
  if (meter === 'credits') return new RegExp(`^\\d+(\\.\\d{1,${CREDIT_DECIMALS}})?$`).test(text) ? normalise(text) : null;
  const suffix = /^(\d+(?:\.\d+)?)([kKmM])$/.exec(text);
  if (suffix !== null && meter === 'tokens') {
    const factor = TOKEN_SUFFIX[(suffix[2] ?? 'k').toLowerCase() as 'k' | 'm'];
    const whole = Math.round(Number(suffix[1]) * factor);
    return Number.isSafeInteger(whole) && Math.abs(Number(suffix[1]) * factor - whole) < 1e-6 ? String(whole) : null;
  }
  return /^\d+$/.test(text) ? normalise(text) : null;
}

/** An amount without the zeros that mean nothing: "0500" is "500", "2.50" is "2.5". */
function normalise(text: string): string {
  const [whole = '0', fraction = ''] = text.split('.');
  const trimmed = fraction.replace(/0+$/, '');
  const head = whole.replace(/^0+(?=\d)/, '');
  return trimmed === '' ? head : `${head}.${trimmed}`;
}

// ---------------------------------------------------------------- the rules

/** What is wrong with a draft, by the member of the request it is about. */
export type Faults = Partial<Record<'subject' | 'amount' | 'counted_from' | 'warn_percent' | 'models' | 'note', string>>;

/** The most characters a note holds: the API's. */
export const NOTE_MOST_CHARS = 500;

/** A day written as `YYYY-MM-DD` that is a day. */
function isDay(text: string): boolean {
  return /^\d{4}-\d{2}-\d{2}$/.test(text) && !Number.isNaN(Date.parse(`${text}T00:00:00Z`)) && new Date(`${text}T00:00:00Z`).toISOString().slice(0, 10) === text;
}

/** Whether a model pattern is one the API takes: no space in it, and a group is `@` and a name of letters, digits, `-` and `_`. */
export function modelPatternFault(pattern: string): 'space' | 'group' | null {
  if (/\s/.test(pattern)) return 'space';
  if (pattern.startsWith('@') && !/^@[a-z0-9][a-z0-9_-]{0,39}$/.test(pattern)) return 'group';
  return null;
}

/** What a person typed for a model, as a pattern: a family written with an ellipsis ends in `*`, everything else is as typed and in lower case. */
export function modelPattern(typed: string): string {
  const text = typed.trim().toLowerCase();
  return text.endsWith('…') ? `${text.slice(0, -1)}*` : text;
}

/** What is wrong with a draft before it is sent, said in words, by what it is about. */
export function check(draft: CapDraft, t: TFunction): Faults {
  const faults: Faults = {};
  if (draft.who === 'user' && draft.userId === '') faults.subject = t('caps.form.faults.person');
  if ((draft.who === 'group' || draft.who === 'eachGroup') && draft.groupId === '') faults.subject = t('caps.form.faults.group');
  if ((draft.who === 'members' || draft.who === 'eachMembers') && draft.membersId === '') faults.subject = t('caps.form.faults.holder');
  if (!draft.noLimit) {
    if (draft.amount.trim() === '') faults.amount = t('caps.form.faults.amountEmpty');
    else if (amountText(draft.amount, draft.meter) === null) {
      faults.amount = draft.meter === 'credits' ? t('caps.form.faults.credits', { places: CREDIT_DECIMALS }) : draft.meter === 'tokens' ? t('caps.form.faults.tokens') : t('caps.form.faults.turns');
    }
  }
  if (draft.meter !== 'running') {
    if (draft.period === 'total' && draft.countedFrom !== '' && !isDay(draft.countedFrom)) faults.counted_from = t('caps.form.faults.day');
    if (draft.warn.trim() !== '') {
      const percent = Number(draft.warn);
      if (!Number.isInteger(percent) || percent < WARN_PERCENT.least || percent > WARN_PERCENT.most) faults.warn_percent = t('caps.form.faults.warn', WARN_PERCENT);
    }
  }
  const bad = draft.models.find((pattern) => modelPatternFault(pattern) !== null);
  if (bad !== undefined) faults.models = modelPatternFault(bad) === 'group' ? t('caps.form.faults.groupName') : t('caps.form.faults.space');
  if (draft.note.length > NOTE_MOST_CHARS) faults.note = t('caps.form.faults.note', { count: NOTE_MOST_CHARS });
  return faults;
}

// ---------------------------------------------------------------- on the wire

/** The words the API puts at the members it refuses, for the member of the form each is about. */
export function faultTarget(path: string): keyof Faults | null {
  const head = path.split(/[.[]/)[0] ?? '';
  switch (head) {
    case 'subject':
    case 'split':
      return 'subject';
    case 'amount':
      return 'amount';
    case 'counted_from':
      return 'counted_from';
    case 'warn_percent':
      return 'warn_percent';
    case 'models':
      return 'models';
    case 'note':
      return 'note';
    default:
      return null;
  }
}

/** The request that makes the cap a draft describes, on a place or on a provider. */
export function inputOf(draft: CapDraft, target: { nodeId?: string | undefined; providerId?: string | undefined }): CapInput {
  const running = draft.meter === 'running';
  const amount = draft.noLimit ? null : amountText(draft.amount, draft.meter);
  return {
    ...(target.nodeId === undefined ? {} : { node_id: target.nodeId }),
    ...(target.providerId === undefined ? {} : { provider_id: target.providerId }),
    subject: subjectOf(draft),
    split: splitOf(draft.who),
    models: draft.models,
    tiers: draft.tiers,
    harness_kinds: draft.harnessKinds,
    meter: draft.meter,
    amount,
    ...(running ? {} : { period: draft.period }),
    ...(running || draft.period !== 'total' || draft.countedFrom === '' ? {} : { counted_from: draft.countedFrom }),
    ...(running ? {} : { at_limit: draft.atLimit, uncounted: draft.uncounted, warn_percent: draft.warn.trim() === '' ? null : Number(draft.warn) }),
    ...(draft.note === '' ? {} : { note: draft.note }),
  };
}

/** The draft a cap that exists comes to, for changing it. */
export function draftOf(cap: UsageCap): CapDraft {
  return emptyDraft({
    who: whoOf(cap),
    userId: cap.subject.kind === 'user' ? cap.subject.user.id : '',
    groupId: cap.subject.kind === 'group' ? cap.subject.group.id : '',
    role: cap.subject.kind === 'role' ? cap.subject.role : 'developer',
    roleAt: cap.subject.kind === 'role' ? cap.subject.at?.id ?? '' : '',
    membersId: cap.subject.kind === 'members' ? cap.subject.node.id : '',
    meter: cap.meter,
    amount: cap.amount ?? '',
    noLimit: cap.amount === null,
    period: cap.period ?? 'day',
    countedFrom: cap.counted_from ?? '',
    models: [...cap.models],
    harnessKinds: [...cap.harness_kinds],
    tiers: [...cap.tiers],
    warn: cap.warn_percent === null ? '' : String(cap.warn_percent),
    atLimit: cap.at_limit,
    uncounted: cap.uncounted,
    note: cap.note,
  });
}

/** The same lists, whatever order they were written in. */
const sameList = (one: readonly string[], other: readonly string[]): boolean => one.length === other.length && [...one].sort().join('\n') === [...other].sort().join('\n');

/** What a person changed in a cap: only what differs from the cap as it stands, against the revision they saw. */
export function updateOf(draft: CapDraft, cap: UsageCap): UsageCapUpdateBody {
  const amount = draft.noLimit ? null : amountText(draft.amount, draft.meter);
  const running = cap.meter === 'running';
  const warn = draft.warn.trim() === '' ? null : Number(draft.warn);
  return {
    expected_revision: cap.revision,
    ...(amount === cap.amount ? {} : { amount }),
    ...(sameList(draft.models, cap.models) ? {} : { models: draft.models }),
    ...(sameList(draft.tiers, cap.tiers) ? {} : { tiers: draft.tiers }),
    ...(sameList(draft.harnessKinds, cap.harness_kinds) ? {} : { harness_kinds: draft.harnessKinds }),
    ...(running || draft.atLimit === cap.at_limit ? {} : { at_limit: draft.atLimit }),
    ...(running || draft.uncounted === cap.uncounted ? {} : { uncounted: draft.uncounted }),
    ...(running || warn === cap.warn_percent ? {} : { warn_percent: warn }),
    ...(running || cap.period !== 'total' || (draft.countedFrom || null) === cap.counted_from ? {} : { counted_from: draft.countedFrom === '' ? null : draft.countedFrom }),
    ...(draft.note === cap.note ? {} : { note: draft.note }),
  };
}

/** Whether a change says anything at all besides the revision. */
export function isChange(update: UsageCapUpdateBody): boolean {
  return Object.keys(update).some((member) => member !== 'expected_revision');
}
