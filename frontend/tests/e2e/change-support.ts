/** What the end-to-end tests of changing where and on what work runs share: the controls of the part of the fake API that plans and carries changes out, and the names it is steered by. */
import { expect, type Page } from '@playwright/test';
import type { Job } from '../../src/generated/api-types.ts';

import { id as fixtureId, KIND } from '../doubles/fixtures.ts';
import { ACME, emit, playScene, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** The second host of the team: a machine of Ada's that takes no new environments until a test opens it. */
export const BUILD_HOST = fixtureId(KIND.host, 2);
export const HOST_ONE = fixtureId(KIND.host, 1);

async function control(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/change${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`the fake API refused change${path}: ${response.status} ${await response.text()}`);
  return response.status === 204 ? null : response.json();
}

export interface JobRow {
  id: string;
  kind: string;
  state: string;
  node_id: string | null;
  environment_id: string | null;
  progress: { percent: number; phase: string; phases: { name: string; state: string }[] };
  error: { code: string; phase: string; undone: boolean } | null;
}

export const change = {
  /** Publishes the current state of a background read through the same native job feed as other work. */
  measurement: (value: { id: string; state?: 'running' | 'failed' | 'cancelled' | 'succeeded'; phase?: string; files?: number; bytes?: number; startedAt?: string; updatedAt?: string; stall?: number; matches?: boolean; checkedAt?: string; message?: string }) => control('/measurement', value) as Promise<Job>,
  /** Jobs advance by themselves (`auto`) or only when a test steps them (`manual`). */
  mode: (mode: 'auto' | 'manual') => control('/mode', { mode }),
  /** Every job that is under way takes `count` steps. */
  step: (count = 1) => control('/step', { count }) as Promise<{ jobs: JobRow[] }>,
  /** Every job that is under way is carried to its end. */
  finish: () => control('/finish') as Promise<{ jobs: JobRow[] }>,
  /** The next job that reaches this phase ends there, and takes back what it made. */
  fail: (phase: string, code: string, message: string, undone = true) => control('/fail', { phase, code, message, undone }),
  /** Every plan that is kept stops being good. */
  expire: () => control('/expire'),
  /** The clock the plans are dated by runs this many minutes ahead of the real one: for a page whose clock was moved forward. */
  clock: (minutes: number) => control('/clock', { minutes }),
  jobs: () => control('/jobs') as Promise<JobRow[]>,
  /** These threads are taken not to be visible to the caller: counted and not named. */
  hide: (threads: string[]) => control('/hide', { threads }),
  /** An environment has been idle this long, for a recycle. */
  idle: (minutes: Record<string, number>) => control('/idle', minutes),
  /** A host declares this much memory, in megabytes. */
  declare: (megabytes: Record<string, number>) => control('/declare', megabytes),
  environment: (body: { id: string; status?: string; updates?: number; host?: string; manages?: string[] }) => control('/environment', body),
  host: (body: { id: string; schedulable?: boolean; when_full?: 'warn' | 'block' }) => control('/host', body),
  /** A model of a provider is switched on or off behind the back of a list that was read before. */
  model: (provider: string, model: string, enabled: boolean) => control('/model', { provider, model, enabled }),
  /** Somebody else's switch of a thread is under way. */
  hold: (thread: string, provider: string) => control('/hold', { thread, provider }) as Promise<{ job: string }>,
  /** The people who may use a provider. */
  uses: async (provider: string) => ((await control('/uses', { provider })) as { uses: string[] }).uses,
  /** A thread is bound to another environment. */
  rebind: (thread: string, environment: string) => control('/rebind', { thread, environment }),
  /** Open side threads, made at once. */
  sides: (thread: string, count: number) => control('/sides', { thread, count }),
};

/** The dialog of a change, by the name it carries. */
export const wizard = (page: Page, name: string | RegExp) => page.getByRole('dialog', { name });

/** What a page says of a thread's model, from the box under its conversation. */
export async function expectBoxFor(page: Page, agent: string): Promise<void> {
  await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toHaveAttribute('placeholder', new RegExp(`Message ${agent}`));
}

/**
 * Where the screenshots of a run are written, when it is asked for them with `ORCH_SHOTS`: one picture of each step in each
 * theme, to look at what a screen came to. Without it nothing is taken, and the tests are the same.
 */
export async function shot(page: Page, name: string): Promise<void> {
  const directory = process.env.ORCH_SHOTS;
  if (directory === undefined || directory === '') return;
  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.screenshot({ path: `${directory}/${name}-${theme}.png` });
  }
  await page.emulateMedia({ colorScheme: 'light' });
}

/** The thread that its provider has held back, and the one with a conversation to fork from. */
export const BLOCKED = ACME.idleThread;
export const MAIN = ACME.mainThread;

export const conversation = (page: Page) => page.getByRole('log', { name: 'Conversation' });

/** The thread that its provider holds back, signed in as Ada, with the way to switch it in view. */
export async function openBlocked(page: Page): Promise<void> {
  await signIn(page);
  await page.goto(`/thread/${BLOCKED}`);
  await expect(page.getByRole('button', { name: 'Switch provider…' })).toBeVisible();
}

/** The main thread, idle, after two turns: a conversation to fork from the middle of. */
export async function openHistory(page: Page): Promise<void> {
  await emit(MAIN, { thread: { state: 'idle' } });
  await playScene(MAIN, 'history');
  await signIn(page);
  await page.goto(`/thread/${MAIN}`);
  await expect(conversation(page).getByText('Please fix it, and run the tests.')).toBeVisible();
}

/** Presses Tab until the focus stands on the control with this name (its label, else its words): what a person does who has no pointer. */
export async function tabTo(page: Page, name: string | RegExp, limit = 60): Promise<void> {
  for (let presses = 0; presses < limit; presses += 1) {
    await page.keyboard.press('Tab');
    const label = await page.evaluate(() => {
      const element = document.activeElement;
      if (element === null) return '';
      const labelledBy = element.getAttribute('aria-labelledby');
      const label = labelledBy === null ? undefined : labelledBy.split(/\s+/).map((id) => document.getElementById(id)?.textContent ?? '').join(' ');
      return (element.getAttribute('aria-label') ?? label ?? element.textContent).replace(/\s+/g, ' ').trim();
    });
    if (typeof name === 'string' ? label === name : name.test(label)) return;
  }
  throw new Error(`the focus did not reach ${String(name)} in ${limit} presses of Tab`);
}
