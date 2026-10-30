/** The real settings screen uses compact rows and native wire shapes; injected data is presentation proof, not an ACL double. */
import { expect, type Locator, type Page, test } from '@playwright/test';
import type { NotificationPrefs, ResolvedSetting, SettingDef, SettingReach } from '../../src/generated/api-types.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chineseWords from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn } from './support.ts';

const definition = (index: number): SettingDef => ({ key: `review.feature${index}`, title: `Feature ${index}`, description: `Keep feature ${index} available.`,
  class: 'parameter', scopes: ['org'], group: index % 2 === 0 ? 'agent' : 'confinement', merge: 'replace', schema: { type: 'boolean' },
  default: false, example: true, text_key: null, keywords: [] });
const modes: SettingDef = { ...definition(145), key: 'approval.allowed_modes', title: 'Allowed approval modes', class: 'policy', merge: 'intersect',
  schema: { type: 'array', items: { enum: ['ask', 'on_request', 'auto', 'bypass'] }, uniqueItems: true }, default: ['ask', 'on_request', 'auto', 'bypass'] };
const delay: SettingDef = { ...definition(144), key: 'env.idle_stop_minutes', title: 'Idle stop', description: 'Stop an environment after this many idle minutes.',
  default: 60, example: 90, schema: { type: 'integer', minimum: 0 }, display: { unit: 'minutes', zero: 'never' } };
const inherited: ResolvedSetting = { key: delay.key, value: 60, own: null, writable: true, locked_by: null,
  source: { scope: 'org', node: { id: ACME.org, kind: 'org', name: 'Acme', redacted: false } }, parts: [] };
const reach: SettingReach = { applies: { orgs: 0, projects: 1, topics: 1, threads: 7 }, overrides: {
  items: [{ id: ACME.fixLogin, kind: 'topic', name: 'login', redacted: false }], next_cursor: null,
} };

/** Wait for the actual fonts, finite transitions and layout, without changing product animation/style. */
async function settled(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().filter((animation) => {
      const end = animation.effect?.getComputedTiming().endTime;
      return animation.playState === 'running' && typeof end === 'number' && Number.isFinite(end);
    }).map((animation) => animation.finished.catch(() => undefined)));
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  });
}

/** Readonly rows may wrap their value column; their height must come from the text and controls they actually contain. */
async function readonlyGeometry(rows: Locator) {
  return rows.evaluateAll((elements) => {
    const rounding = 1; // One CSS pixel for subpixel boxes/borders, not a row-height allowance.
    const rootFont = Number.parseFloat(getComputedStyle(document.documentElement).fontSize);
    const box = (element: Element) => {
      const { left, right, top, bottom, width, height } = element.getBoundingClientRect();
      return { left, right, top, bottom, width, height };
    };
    const separate = (one: ReturnType<typeof box>, other: ReturnType<typeof box>) =>
      one.right <= other.left + rounding || other.right <= one.left + rounding || one.bottom <= other.top + rounding || other.bottom <= one.top + rounding;
    const inside = (inner: ReturnType<typeof box>, outer: ReturnType<typeof box>) =>
      inner.left >= outer.left - rounding && inner.right <= outer.right + rounding && inner.top >= outer.top - rounding && inner.bottom <= outer.bottom + rounding;
    return elements.map((element) => {
      if (!(element instanceof HTMLElement)) throw new Error('A setting row must be an element');
      const name = element.children[0], value = element.children[1];
      const description = element.querySelector<HTMLElement>('[data-setting-description]');
      const title = name?.firstElementChild;
      const valueText = value?.firstElementChild?.firstElementChild;
      if (!(name instanceof HTMLElement) || !(value instanceof HTMLElement) || !(title instanceof HTMLElement)
        || !(valueText instanceof HTMLElement) || description === null) throw new Error('A readonly row lost its name/description/value structure');
      const frame = box(element), nameBox = box(name), valueBox = box(value), titleBox = box(title), descriptionBox = box(description);
      const style = getComputedStyle(element), descriptionStyle = getComputedStyle(description);
      const topInset = Number.parseFloat(style.paddingTop) + Number.parseFloat(style.borderTopWidth);
      const bottomInset = Number.parseFloat(style.paddingBottom) + Number.parseFloat(style.borderBottomWidth);
      const content = { ...frame, left: frame.left + Number.parseFloat(style.paddingLeft) + Number.parseFloat(style.borderLeftWidth),
        right: frame.right - Number.parseFloat(style.paddingRight) - Number.parseFloat(style.borderRightWidth), top: frame.top + topInset, bottom: frame.bottom - bottomInset };
      const slots = Array.from(value.children).map(box).filter((rect) => rect.width > 0 && rect.height > 0);
      const targets = Array.from(value.querySelectorAll('button,a[href]')).map(box).filter((rect) => rect.width > 0 && rect.height > 0);
      const sibling = element.nextElementSibling;
      const next = sibling?.matches('[data-setting-row]') ? box(sibling) : null;
      return { key: element.dataset.setting, height: frame.height,
        intrinsicHeight: Math.max(nameBox.bottom, valueBox.bottom) - Math.min(nameBox.top, valueBox.top) + topInset + bottomInset,
        nameHeight: nameBox.height, nameContentHeight: titleBox.height + descriptionBox.height,
        valueHeight: valueBox.height, valueContentHeight: Math.max(...slots.map((rect) => rect.bottom)) - Math.min(...slots.map((rect) => rect.top)),
        descriptionHeight: descriptionBox.height, descriptionLine: Number.parseFloat(descriptionStyle.lineHeight),
        titleLine: Number.parseFloat(getComputedStyle(title).lineHeight), valueLine: Number.parseFloat(getComputedStyle(valueText).lineHeight),
        // Both text-base and text-sm use 1.25rem leading in the approved shared tokens.
        tokenLine: rootFont * 1.25,
        visible: frame.width > 0 && frame.height > 0 && descriptionBox.width > 0 && descriptionStyle.display !== 'none'
          && descriptionStyle.visibility === 'visible' && Number.parseFloat(descriptionStyle.opacity) > 0
          && element.closest('[hidden],[inert],[aria-hidden="true"]') === null,
        descriptionBelowName: descriptionBox.top >= titleBox.bottom - rounding,
        descriptionNotClippedVertically: description.scrollHeight <= description.clientHeight + rounding,
        contained: inside(nameBox, content) && inside(valueBox, content) && inside(descriptionBox, nameBox) && slots.every((rect) => inside(rect, valueBox))
          && targets.every((rect) => inside(rect, valueBox)),
        noOverlap: separate(nameBox, valueBox) && targets.every((rect, index) => targets.slice(index + 1).every((other) => separate(rect, other)))
          && (next === null || frame.bottom <= next.top + rounding),
      };
    });
  });
}

/** Editor.tsx uses its own content-box width for @min-[48rem], not the browser viewport. */
async function contentsMode(contents: Locator) {
  return contents.evaluate((element) => {
    let container = element.parentElement;
    while (container !== null && getComputedStyle(container).containerType === 'normal') container = container.parentElement;
    if (container === null) throw new Error('Settings contents lost its query container');
    const style = getComputedStyle(container);
    const width = container.getBoundingClientRect().width - Number.parseFloat(style.paddingLeft) - Number.parseFloat(style.paddingRight)
      - Number.parseFloat(style.borderLeftWidth) - Number.parseFloat(style.borderRightWidth);
    const breakpoint = 48 * Number.parseFloat(getComputedStyle(document.documentElement).fontSize);
    return { width, breakpoint, wide: width >= breakpoint, position: getComputedStyle(element).position };
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`notifications keep a real section gap at desktop and phone widths in ${locale}/${theme}`, async ({ page }) => {
    await scenario(); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const prefs: NotificationPrefs = { approvals_always: false, chat_node_level: 'mentions', chat_direct_level: 'mentions', quiet_hours: null, pause_until: null, revision: 1 };
    const writes: string[] = [];
    await page.route('**/api/v1/me/notification-prefs', (route) => {
      if (route.request().method() !== 'GET') writes.push(route.request().method());
      return route.fulfill({ json: prefs });
    });
    await page.goto('/settings/notifications');
    const content = page.locator('[data-settings-content]');
    await expect(content.locator(':scope > section')).toHaveCount(5);
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      const geometry = await content.evaluate((element) => {
        const style = getComputedStyle(element);
        const sections = Array.from(element.children).filter((child) => child.tagName === 'SECTION').map((child) => child.getBoundingClientRect());
        return { display: style.display, direction: style.flexDirection, gap: Number.parseFloat(style.rowGap), rootFont: Number.parseFloat(getComputedStyle(document.documentElement).fontSize),
          distances: sections.slice(1).map((box, index) => box.top - (sections[index]?.bottom ?? box.top)) };
      });
      expect(geometry.display).toBe('flex'); expect(geometry.direction).toBe('column');
      // Step six is 1.5rem. Compare actual geometry, allowing one CSS pixel for box rounding.
      expect(geometry.gap).toBeCloseTo(geometry.rootFont * 1.5, 1);
      expect(geometry.distances).toHaveLength(4);
      for (const distance of geometry.distances) expect(distance).toBeGreaterThanOrEqual(geometry.gap - 1);
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
    expect(writes).toEqual([]);
  });

  test(`146 rows, one view-only notice, guarded save/Undo and lazy reach at both widths in ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const chinese = locale === 'zh-CN';
    const words = chinese ? chineseWords : english;
    const definitions = [...Array.from({ length: 144 }, (_, index) => definition(index)), delay, modes];
    const readOnly = definitions.filter((def) => def.key !== delay.key).map((def): ResolvedSetting => ({ key: def.key, value: def.default, own: null,
      writable: false, locked_by: null, source: { scope: 'default', node: null }, parts: [] }));
    let current = inherited;
    let listReads = 0;
    let reachReads = 0;
    const writes: { method: string; body: unknown; guard: string | null }[] = [];
    await page.route('**/api/v1/settings/registry', (route) => route.fulfill({ json: definitions }));
    await page.route(`**/api/v1/nodes/${ACME.research}/settings`, (route) => { listReads += 1; return route.fulfill({ json: [...readOnly, current] }); });
    const settingPath = `/api/v1/nodes/${ACME.research}/settings/${delay.key}`;
    // Match the native child endpoint as a path; a single glob star does not cross its slash.
    await page.route((url) => url.pathname === settingPath || url.pathname === `${settingPath}/reach`, async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (url.pathname.endsWith('/reach')) {
        expect(request.method()).toBe('GET');
        expect(url.searchParams.get('limit')).toBe('50');
        reachReads += 1; await route.fulfill({ json: reach }); return;
      }
      const body: unknown = request.postData() === null ? null : request.postDataJSON();
      writes.push({ method: request.method(), body, guard: url.searchParams.get('expected_revision') });
      if (request.method() === 'DELETE') current = inherited;
      else if (typeof body === 'object' && body !== null && 'value' in body && typeof body.value === 'number') {
        current = { ...inherited, value: body.value, own: { value: body.value, locked: false, revision: 1 },
          source: { scope: 'org', node: { id: ACME.research, kind: 'org', name: 'Research', redacted: false } } };
      } else throw new Error('The setting write has no numeric value');
      await route.fulfill({ json: current });
    });
    await page.goto('/org/acme.research/settings/policies');
    const rows = page.locator('[data-setting-row]');
    await expect(rows).toHaveCount(146);
    await expect(page.locator('[data-read-only-settings]')).toHaveCount(1);
    const readonly = page.locator('[data-setting-readonly]');
    await expect(readonly).toHaveCount(readOnly.length);
    await expect(readonly.locator('[data-setting-description]')).toHaveCount(readOnly.length);
    await expect(rows.locator('[data-setting-description]')).toHaveCount(definitions.length);
    // Supplied descriptions remain visible in readonly rows as well as editable rows.
    // Extension definitions keep their words; the known approval key uses its existing locale catalogue.
    const expectedDescriptions = definitions.filter((def) => def.key !== delay.key).map((def) => ({ key: def.key,
      text: def.key === modes.key ? words.settingRegistry.approval.allowed_modes.description : def.description,
      fullText: def.key === modes.key ? words.settingRegistry.approval.allowed_modes.description : def.description,
    })).sort((one, other) => one.key.localeCompare(other.key));
    const suppliedDescriptions = await readonly.evaluateAll((elements) => elements.map((element) => {
      const description = element.querySelector('[data-setting-description]');
      return { key: element.getAttribute('data-setting'), text: description?.textContent, fullText: description?.getAttribute('data-overflow-text') };
    }));
    expect(suppliedDescriptions.sort((one, other) => (one.key ?? '').localeCompare(other.key ?? ''))).toEqual(expectedDescriptions);
    await expect(readonly.locator('input,textarea,select,[contenteditable="true"],[role="switch"],[role="checkbox"],[role="radio"],[role="combobox"],[data-setting-editor-trigger]')).toHaveCount(0);
    const approval = page.locator('[data-setting="approval.allowed_modes"]');
    await expect(approval).not.toContainText('on_request');
    await expect(approval).not.toContainText('[');
    await expect(approval.getByRole('listitem')).toHaveCount(4);
    const row = page.locator(`[data-setting="${delay.key}"]`);
    await expect(row.getByRole('spinbutton')).toHaveValue('60');
    await expect(row.getByRole('button', { name: chinese ? '保存' : 'Save', exact: true })).toHaveCount(0);
    await expect(row.getByRole('button', { name: /^(Reset|重置)/ })).toHaveCount(0);
    expect(reachReads).toBe(0);

    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await settled(page);
      const geometry = await readonlyGeometry(readonly);
      expect(geometry).toHaveLength(readOnly.length);
      for (const measured of geometry) {
        const label = `${measured.key ?? 'Missing setting key'} at ${width}px`;
        expect(measured.visible, label).toBe(true);
        expect(measured.descriptionBelowName, label).toBe(true);
        expect(measured.descriptionNotClippedVertically, label).toBe(true);
        expect(measured.contained, label).toBe(true); expect(measured.noOverlap, label).toBe(true);
        for (const line of [measured.descriptionLine, measured.titleLine, measured.valueLine]) expect(Math.abs(line - measured.tokenLine), label).toBeLessThanOrEqual(1);
        expect(Math.abs(measured.descriptionHeight - measured.descriptionLine), label).toBeLessThanOrEqual(1);
        expect(Math.abs(measured.nameHeight - measured.nameContentHeight), label).toBeLessThanOrEqual(1);
        expect(Math.abs(measured.valueHeight - measured.valueContentHeight), label).toBeLessThanOrEqual(1);
        expect(Math.abs(measured.height - measured.intrinsicHeight), label).toBeLessThanOrEqual(1);
      }
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      const contents = page.locator('[data-settings-contents]');
      const group = page.getByRole('combobox', { name: words.settings.editor.groupsNav, exact: true });
      const presentation = await contentsMode(contents);
      expect(presentation.position).toBe('sticky');
      // CSS links remain in the DOM while the narrow locator is shown; both must reach this exact section.
      const destination = contents.locator('a[href]').last();
      const href = await destination.getAttribute('href');
      if (href?.startsWith('#') !== true) throw new Error('The contents link must name its settings section');
      if (presentation.wide) {
        await expect(contents).toBeVisible();
        await expect(group).toBeHidden();
        await destination.focus(); await destination.press('Enter');
      } else {
        await expect(contents).toBeHidden();
        await expect(group).toBeVisible();
        await group.focus();
        await expect(page.getByRole('listbox', { name: words.settings.editor.groupsNav, exact: true })).toBeVisible();
        await group.press('End'); await group.press('Enter');
      }
      await expect(page.locator('[id]:focus')).toHaveCount(1);
      await expect(page.locator('[id]:focus')).toHaveAttribute('id', href.slice(1));
      await expect(destination).toHaveAttribute('aria-current', 'location');
      expect(writes).toEqual([]); expect(listReads).toBe(1); expect(reachReads).toBe(0);
      await page.mouse.move(0, 0); await settled(page);
      const screenshot = info.outputPath(`settings-rows-${locale}-${theme}-${width}.png`);
      await page.screenshot({ path: screenshot, fullPage: false, caret: 'hide' });
      await info.attach(`settings-rows-${locale}-${theme}-${width}`, { path: screenshot, contentType: 'image/png' });
      await info.attach(`settings-rows-geometry-${width}`, { body: JSON.stringify({ viewport: width, presentation, rows: geometry }, null, 2), contentType: 'application/json' });
    }
    await row.getByRole('spinbutton').fill('90');
    await row.getByRole('spinbutton').press('Enter');
    await expect(row.getByText(chinese ? '已保存' : 'Saved', { exact: true })).toBeVisible();
    expect(writes).toEqual([{ method: 'PUT', body: { value: 90, expected_revision: 0 }, guard: null }]);
    expect(listReads).toBe(1);
    await row.getByRole('button', { name: /^(Undo the change to|撤销对)/ }).click();
    await expect(row.getByRole('spinbutton')).toHaveValue('60');
    expect(writes).toEqual([
      { method: 'PUT', body: { value: 90, expected_revision: 0 }, guard: null },
      { method: 'DELETE', body: null, guard: '1' },
    ]);
    expect(listReads).toBe(1);
    await row.getByRole('button', { name: chinese ? '适用位置' : 'Where it applies', exact: true }).click();
    const details = page.getByRole('region', { name: chinese ? '此值适用的位置' : 'Where this value applies' });
    await expect(details).toContainText(chinese ? '7 个线程' : '7 threads');
    await expect(details.getByRole('link', { name: 'login', exact: true })).toHaveAttribute('href', `/topic/${ACME.fixLogin}?tab=settings&section=policies&setting=${delay.key}`);
    expect(reachReads).toBe(1);
    expect(listReads).toBe(1);
  });
}
