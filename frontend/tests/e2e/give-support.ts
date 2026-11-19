/** What the end-to-end tests of giving share: steering the fake API's access and shares, a second person in a browser of their own, and the few places on the screen that every test looks at. */
import { expect, type Browser, type BrowserContext, type Page } from '@playwright/test';

import { ACME, PEOPLE, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** Somebody of no org of the team's tree: named from outside, where the person naming may. */
export const ZOE = { login: 'zoe', password: 'beyond the wall', name: 'Zoe Outsider' } as const;

/** A request only the fake API has, which steers what the access pages meet (`path` begins with a slash and a section of it: `/serve`, `/shares/snapshot`). */
export async function steer(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/access${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status} ${await response.text()}`);
  return response.status === 204 ? null : response.json();
}

/** The changes of pages the fake API was asked for and made, newest last: what a form sent. */
export async function sent(): Promise<{ path: string; id: string; change: { name?: { choice: string; holders: unknown[]; export?: boolean }[]; rows?: { permission: string; add?: { holder: { kind: string; id: string } }[] }[] } }[]> {
  return (await steer('/sent')) as never;
}

/** The dialog of the permission page of a thing. */
export const dialog = (page: Page) => page.getByRole('dialog', { name: /^Access to / });

/** A line of the short form, by its heading. */
export const line = (page: Page, name: string) => dialog(page).getByRole('group', { name });

/** Opens a browser of its own for a second person, signed in and at a path. */
export async function second(browser: Browser, baseURL: string | undefined, path: string, person: { login: string; password: string }): Promise<{ page: Page; context: BrowserContext }> {
  const context = await browser.newContext({ baseURL: baseURL ?? 'http://127.0.0.1:41872', locale: 'en-US', timezoneId: 'UTC' });
  const page = await context.newPage();
  await signIn(page, person);
  await page.goto(path);
  return { page, context };
}

/** Goes to a place and opens its Share from the menu of its heading. */
export async function openShare(page: Page, path: string, thing: string): Promise<void> {
  await page.goto(path);
  await page.getByRole('button', { name: `Actions for ${thing}` }).first().click();
  await page.getByRole('menuitem', { name: 'Share…' }).click();
  await expect(dialog(page)).toBeVisible();
}

/** Signs in as a person and opens the Share of a place. */
export async function shareAs(page: Page, person: { login: string; password: string }, path: string, thing: string): Promise<void> {
  await signIn(page, person);
  await openShare(page, path, thing);
}

/** The field that adds people, in the short form. */
export const field = (page: Page) => dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' });

/** Finds a person in the field and stages them: `typed` is what is typed (the first letters of the name unless said), `who` the name of the option chosen. */
export async function stage(page: Page, who: string, typed: string = who.slice(0, 4).trim()): Promise<void> {
  await field(page).fill(typed);
  await page.getByRole('option', { name: new RegExp(who) }).click();
}

/** Opens Share from the thread's own header. */
export async function shareThread(page: Page): Promise<void> {
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Share', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
}

/** Chooses from the control of what is shared, on a place: "<Place> and everything in it" or "Only <place> itself". */
export async function options(page: Page): Promise<void> {
  const toggle = dialog(page).getByRole('button', { name: 'Options', exact: true });
  if (await toggle.getAttribute('aria-expanded') !== 'true') await toggle.click();
}

export async function reach(page: Page, option: string | RegExp): Promise<void> {
  await options(page);
  await dialog(page).getByRole('combobox', { name: 'What is shared' }).click();
  await page.getByRole('option', { name: option }).click();
}

/** Chooses one of the three choices in the form. */
export async function choose(page: Page, choice: string): Promise<void> {
  await dialog(page).getByRole('combobox', { name: 'What they can do' }).click();
  await page.getByRole('option', { name: choice }).click();
}

/** Presses Add. */
export async function add(page: Page): Promise<void> {
  await dialog(page).getByRole('button', { name: 'Add', exact: true }).click();
}

/** Opens a tab of the dialog. */
export async function tab(page: Page, name: string): Promise<void> {
  await dialog(page).getByRole('tab', { name }).click();
}

export { ACME, PEOPLE };
