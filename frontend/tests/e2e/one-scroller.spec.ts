/** One scroll bar a pane: a pane scrolls in one place. The check of the screens: on each screen of the fake world,
 * on a desk and on a phone, no region that scrolls lies inside another that scrolls the same way, where the inner one
 * is a pane of its own. What counts as a pane: an inner scroller at least half as tall (or wide) as the room of the
 * outer one. A smaller box with its own bound, such as a code block or a list in a card, is part of what the pane
 * shows and leaves the pane's own scroll the one that moves it; one of half the room or more competes with it, and
 * the wheel then moves whichever of the two the pointer happens to be over. Both must really scroll at that moment:
 * a region that holds all it shows draws no bar. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

/** The screens of the fake world, each in the state it opens in. */
const SCREENS = [
  '/org/acme',
  '/org/acme/messages',
  '/org/acme/settings/general',
  '/org/acme/settings/policies',
  '/org/acme/settings/members',
  `/project/${ACME.web}`,
  `/project/${ACME.web}?view=board`,
  `/project/${ACME.web}/settings`,
  `/topic/${ACME.fixLogin}`,
  `/topic/${ACME.fixLogin}?tab=settings`,
  `/thread/${ACME.mainThread}`,
  '/inbox',
  '/approvals',
  '/search',
  '/shared',
  '/usage',
  '/help',
  '/resources/providers',
  '/resources/environments',
  '/resources/agents',
  '/settings/profile',
  '/settings/interface',
  '/settings/notifications',
  '/admin/overview',
  '/admin/users',
  '/admin/everything',
] as const;

/** The size from which an inner scroller is a pane of its own: half of the room of the one around it. */
const PANE_SHARE = 0.5;

/** Every scroller that is a pane inside another scroller of the same direction, said by where it is. */
async function nestedPanes(page: Page): Promise<string[]> {
  return page.evaluate((share) => {
    const scrolls = (element: Element, axis: 'x' | 'y'): boolean => {
      const style = getComputedStyle(element);
      const overflow = axis === 'y' ? style.overflowY : style.overflowX;
      if (overflow !== 'auto' && overflow !== 'scroll') return false;
      return axis === 'y' ? element.scrollHeight > element.clientHeight + 1 : element.scrollWidth > element.clientWidth + 1;
    };
    const said = (element: Element): string => {
      const label = element.getAttribute('aria-label') ?? element.getAttribute('data-testid') ?? element.getAttribute('role');
      const classes = Array.from(element.classList).slice(0, 4).join('.');
      return `${element.tagName.toLowerCase()}${label === null ? '' : `[${label}]`}${classes === '' ? '' : `.${classes}`}`;
    };
    const faults: string[] = [];
    for (const element of Array.from(document.querySelectorAll('*'))) {
      for (const axis of ['y', 'x'] as const) {
        if (!scrolls(element, axis)) continue;
        for (let outer = element.parentElement; outer !== null; outer = outer.parentElement) {
          if (!scrolls(outer, axis)) continue;
          const size = axis === 'y' ? element.clientHeight : element.clientWidth;
          const room = axis === 'y' ? outer.clientHeight : outer.clientWidth;
          if (size >= room * share) faults.push(`${axis}: ${said(element)} inside ${said(outer)}`);
          break;
        }
      }
    }
    return faults;
  }, PANE_SHARE);
}

for (const [device, size] of [['a desk', { width: 1280, height: 720 }], ['a phone', { width: 390, height: 844 }]] as const) {
  test(`no pane scrolls inside another the same way, on ${device}`, async ({ page }) => {
    await page.setViewportSize(size);
    await signIn(page);
    const faults: string[] = [];
    for (const screen of SCREENS) {
      await page.goto(screen);
      // A screen is judged once it has drawn what it holds; one part that never arrives does not hold the others back.
      await expect.poll(() => page.locator('[aria-busy="true"]').count(), { timeout: 5_000 }).toBe(0).catch(() => undefined);
      for (const fault of await nestedPanes(page)) faults.push(`${screen} ${fault}`);
    }
    expect(faults).toEqual([]);
  });
}
