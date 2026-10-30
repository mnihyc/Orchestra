/** What the tests of usage share beyond signing in: steering the ledger of the fake API, making a cap through the API as somebody, and what the ledger comes to for a span. */
import type { CapInput } from '../../src/generated/api-types.ts';
import { ACME, PEOPLE } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

export { ACME, PEOPLE };

/** The ids of the places the ledger is about, by name. */
export const PLACES = {
  web: ACME.web,
  fixLogin: ACME.fixLogin,
  infra: ACME.infra,
  evals: ACME.evals,
  org: ACME.org,
  mainThread: ACME.mainThread,
} as const;

/** What the ledger holds for each day, and for the people who use most of it: the same every run. */
export const DAY = {
  /** Everybody together: six uses of six kinds. */
  all: { turns: 6, tokens: 14_400_000, credits: 62.42, estimated: 32.795, unrated: 600_000 },
  /** Ada: the main thread of Fix login on GPT-5.4 and the idle-stop thread on Claude Opus 5, whose rate is an estimate. */
  ada: { turns: 2, tokens: 6_600_000, credits: 27.94, estimated: 8.19, unrated: 0 },
  /** Project Web: Ada's, Grace's and Dora's uses (the first three kinds). */
  web: { turns: 3, tokens: 10_800_000, credits: 53.625, estimated: 24, unrated: 0 },
  /** Dora: the upgrade-deps thread in Web, on GPT-5.5 Codex at the speed Fast (an estimate), and the small model of Evals (an estimate too). */
  dora: { turns: 2, tokens: 4_800_000, credits: 24.605, estimated: 24.605, unrated: 0 },
  /** Dora in Web: the upgrade-deps thread on GPT-5.5 Codex at the speed Fast. */
  doraInWeb: { turns: 1, tokens: 3_600_000, credits: 24, estimated: 24, unrated: 0 },
} as const;

async function control(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/usage${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? null : response.json();
}

/** Writes the ledger anew for this many days back from today. */
export async function writeHistory(days: number): Promise<void> {
  await control('/history', { days });
}

/** One more use, now, of one of the six kinds (A to F), `scale` times as large as that kind is: the caps count it at once. */
export async function useNow(pattern: 'A' | 'B' | 'C' | 'D' | 'E' | 'F' = 'A', scale = 1): Promise<void> {
  await control('/use', { pattern, scale });
}

/** What runs now against a cap on turns at once. */
export async function setRunning(turns: { thread: string | null; user: string; provider: string | null }[]): Promise<void> {
  await control('/running', { turns });
}

/** What waits for room in a cap, in the order it will run. */
export async function setWaiting(cap: string, items: { thread: string | null; author: string | null; since: string; mine: boolean }[]): Promise<void> {
  await control('/waiting', { cap, items });
}

/** Says that somebody let a turn past a cap. */
export async function passed(cap: string, by: string, count?: number): Promise<void> {
  await control('/bypass', { cap, by, ...(count === undefined ? {} : { count }) });
}

/** The system's own switches for estimates and what one unit of the catalogue is worth, or no catalogue in force at all. */
export async function setCatalogue(settings: { estimates?: boolean; scale?: string; fetch_url?: string | null; none?: boolean }): Promise<void> {
  await control('/catalogue', settings);
}

/** Makes a cap through the API as one of the people, the way a script would. */
export async function makeCap(person: { login: string; password: string }, body: Partial<CapInput> & Pick<CapInput, 'meter' | 'amount'>): Promise<{ id: string }> {
  const login = await fetch(`${DOUBLE}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ login: person.login, password: person.password }) });
  const { access_token: token } = (await login.json()) as { access_token: string };
  // A cap on turns at once has no period.
  const period = body.meter === 'running' ? {} : { period: 'day' };
  const made = await fetch(`${DOUBLE}/api/v1/usage/caps`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() },
    body: JSON.stringify({ subject: { kind: 'everyone' }, split: 'shared', ...period, ...body }),
  });
  if (!made.ok) throw new Error(`the cap was refused: ${made.status} ${await made.text()}`);
  return (await made.json()) as { id: string };
}

/** Today, in UTC, and how many days of this year have gone, counting today. */
export function dayOfYear(now: Date = new Date()): number {
  return Math.floor((Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate()) - Date.UTC(now.getUTCFullYear(), 0, 1)) / 86_400_000) + 1;
}

/** The sums a number of days comes to, for a kind of reader from `DAY`. */
export function sum(per: { turns: number; tokens: number; credits: number; estimated: number }, days: number) {
  return {
    turns: per.turns * days,
    tokens: per.tokens * days,
    credits: Math.round(per.credits * days * 1e6) / 1e6,
    estimated: Math.round(per.estimated * days * 1e6) / 1e6,
  };
}
