/** Bounded content readiness; skeletons remain in a failure screenshot rather than being hidden by the tool. */
import type { Page } from '@playwright/test';
import { READY_MS, type CaptureLocale, type CaptureTheme } from './config.ts';
import type { PageRequests } from './requests.ts';
import { bounded } from './deadline.ts';

export async function contentReady(page: Page, locale: CaptureLocale, theme: CaptureTheme): Promise<void> {
  await page.waitForFunction(({ locale, theme }) => {
    const root = document.getElementById('root');
    if (root === null || root.textContent.trim() === '' || document.documentElement.lang !== locale || document.documentElement.dataset.theme !== theme) return false;
    const visible = (element: Element): boolean => {
      const box = element.getBoundingClientRect(); const style = getComputedStyle(element);
      return box.width > 0 && box.height > 0 && box.bottom > 0 && box.right > 0 && box.top < innerHeight && box.left < innerWidth && style.display !== 'none' && style.visibility !== 'hidden' && style.opacity !== '0';
    };
    const content = Array.from(document.querySelectorAll('main,[role="main"],[role="dialog"],h1,h2'))
      .some((element) => visible(element) && element.textContent.trim().length > 0);
    return content && !Array.from(document.querySelectorAll('[aria-busy="true"],.animate-skeleton,[data-skeleton]')).some(visible)
      && document.fonts.status === 'loaded';
  }, { locale, theme }, { timeout: READY_MS });
  await bounded(page.evaluate(() => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done())))));
}

export async function ready(page: Page, meter: PageRequests, locale: CaptureLocale, theme: CaptureTheme): Promise<void> {
  await contentReady(page, locale, theme);
  await meter.quiet();
  // A late lazy view may have begun to load while the last response settled.
  await contentReady(page, locale, theme);
}
