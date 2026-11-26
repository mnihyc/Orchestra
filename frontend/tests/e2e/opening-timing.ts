/** Explain a missed opening budget using public asset names and in-page startup marks. */
import type { Page } from '@playwright/test';

export async function openingTimings(page: Page): Promise<string> {
  const entries = await page.evaluate(() => performance.getEntries().flatMap((entry) => {
    if (entry.entryType === 'mark' && entry.name.startsWith('orch/start/')) return [`${entry.name} @${Math.round(entry.startTime)}`];
    if (entry.entryType !== 'resource') return [];
    const path = new URL(entry.name).pathname;
    if (!path.startsWith('/assets/') && !['/api/v1/auth/session', '/api/v1/auth/login', '/api/v1/me/landing'].includes(path)) return [];
    return [`${path} ${Math.round(entry.startTime)}–${Math.round(entry.startTime + entry.duration)}`];
  }));
  return entries.join('\n');
}
