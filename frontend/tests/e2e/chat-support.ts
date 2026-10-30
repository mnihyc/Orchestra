/** What the end-to-end tests of chats share: steering the fake API's chats, a second person in a browser of their own, and the few places on the screen that every test looks at. */
import type { Browser, BrowserContext, Page } from '@playwright/test';
import { expect } from '@playwright/test';

import { PEOPLE, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** A request only the fake API has, which steers the chats of its world. */
export async function chats(path: string, body: unknown = {}): Promise<Record<string, unknown>> {
  const response = await fetch(`${DOUBLE}/__double/chats${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`the fake API refused /chats${path}: ${response.status} ${await response.text()}`);
  return response.status === 204 ? {} : ((await response.json()) as Record<string, unknown>);
}

/** The person who signs in as the second one: a Developer of Acme with no second step. */
export const SECOND = PEOPLE.dora;

/** Opens a browser of its own for a second person, signed in and at a path. */
export async function secondReader(
  browser: Browser,
  baseURL: string | undefined,
  path: string,
  person: { login: string; password: string } = SECOND,
): Promise<{ page: Page; context: BrowserContext }> {
  const context = await browser.newContext({ baseURL: baseURL ?? 'http://127.0.0.1:41872', locale: 'en-US', timezoneId: 'UTC' });
  const page = await context.newPage();
  await signIn(page, person);
  await page.goto(path);
  return { page, context };
}

/** The box a message is typed in, in a chat's panel or on the Messages screen. */
export const composer = (page: Page) => page.getByRole('textbox', { name: /^Message / });

/** One message of the conversation on screen, found by what it says. */
export const messageSaying = (page: Page, text: string | RegExp) => page.locator('li[data-message]', { hasText: text });

/** Sends a message by typing it and pressing Enter, and waits until it stands in the list. */
export async function sendMessage(page: Page, text: string): Promise<void> {
  const box = composer(page);
  await box.click();
  await box.fill(text);
  await box.press('Enter');
  await expect(messageSaying(page, text).first()).toBeVisible();
}

/** The menu of a message: opened from its toolbar, which shows while the pointer rests on the message. */
export async function openMessageMenu(page: Page, text: string | RegExp): Promise<void> {
  const message = messageSaying(page, text).first();
  await message.hover();
  await message.getByRole('button', { name: /^Actions/ }).click();
}
