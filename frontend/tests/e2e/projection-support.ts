/** Read synthetic test-API projections as the browser's signed-in person; no second login or saved credential. */
import { expect, type Page } from '@playwright/test';
import { signIn } from './support.ts';

export async function signInWithProjectionReader(page: Page): Promise<<T>(path: string) => Promise<T>> {
  const response = page.waitForResponse((reply) => new URL(reply.url()).pathname === '/api/v1/auth/login' && reply.request().method() === 'POST');
  await signIn(page);
  const login = await response;
  expect(login.ok()).toBe(true);
  const body = await login.json() as { access_token: string };
  return async <T>(path: string): Promise<T> => {
    const reply = await page.request.get(path, { headers: { Authorization: `Bearer ${body.access_token}` } });
    expect(reply.ok(), `The test projection ${path} must be authorized`).toBe(true);
    return await reply.json() as T;
  };
}
