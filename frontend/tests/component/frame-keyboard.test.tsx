/** The first keyboard stop skips navigation; area travel ignores hidden and detached views. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { setLocale } from '@/i18n';
import { areaStops, focusArea, mainContent } from '@/shell/areas';
import { SkipMain } from '@/shell/SkipMain';
import { show } from './support';

afterEach(async () => { cleanup(); vi.restoreAllMocks(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('skips the frame and cycles only visible areas in %s', async (locale) => {
  await setLocale(locale);
  // jsdom has no layout. A connected, non-hidden test element occupies one box.
  vi.spyOn(HTMLElement.prototype, 'getClientRects').mockImplementation(function (this: HTMLElement) {
    return [this.getBoundingClientRect()] as unknown as DOMRectList;
  });
  const user = userEvent.setup();
  show(<><SkipMain /><nav data-area-stop aria-label="Rail"><button>Choose org</button></nav>
    <aside data-area-stop hidden><button>Hidden view</button></aside>
    <aside data-area-stop inert><button>Covered view</button></aside>
    <main id="main-content" data-main-area tabIndex={-1}><section data-view-group aria-label="Editor"><input aria-label="Draft" /></section></main>
    <footer data-area-stop tabIndex={-1}>Connection</footer></>);
  await user.tab();
  const skip = screen.getByRole('link', { name: locale === 'en' ? 'Skip to main content' : '跳至主要内容' });
  expect(skip).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(document.getElementById('main-content')).toHaveFocus();
  expect(areaStops().map((element) => element.tagName)).toEqual(['NAV', 'SECTION', 'FOOTER']);
  expect(focusArea(1)).toBe(true);
  expect(screen.getByText('Connection')).toHaveFocus();
  expect(focusArea(1)).toBe(true);
  expect(screen.getByRole('button', { name: 'Choose org' })).toHaveFocus();
  expect(focusArea(-1)).toBe(true);
  expect(screen.getByText('Connection')).toHaveFocus();
  expect(focusArea(-1)).toBe(true);
  expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveFocus();
});

it('skips into the shown sheet when the phone page is covered and inert', async () => {
  vi.spyOn(HTMLElement.prototype, 'getClientRects').mockImplementation(function (this: HTMLElement) { return [this.getBoundingClientRect()] as unknown as DOMRectList; });
  show(<><SkipMain /><main data-main-area inert tabIndex={-1}>Covered page</main><section data-area-stop data-workbench-bar="right" tabIndex={-1} aria-label="Environment">Shown sheet</section></>);
  const sheet = screen.getByRole('region', { name: 'Environment' });
  expect(mainContent()).toBe(sheet);
  expect(areaStops()).toEqual([sheet]);
  const user = userEvent.setup();
  await user.tab(); await user.keyboard('{Enter}');
  expect(sheet).toHaveFocus();
});
