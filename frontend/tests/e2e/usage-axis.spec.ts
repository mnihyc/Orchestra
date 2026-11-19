/** Real uPlot text must fit its canvas after fonts, localized intervals and pane resizing settle. */
import { expect, type Locator, type Page, test } from '@playwright/test';
import type { UsageAnalytics } from '../../src/generated/api-types.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support.ts';

interface Paint {
  text: string;
  x: number;
  y: number;
  left: number;
  right: number;
  top: number;
  bottom: number;
  font: string;
  fill: string;
  align: CanvasTextAlign;
  baseline: CanvasTextBaseline;
}
interface Frame { number: number; labels: Paint[] }
type ObservedWindow = typeof window & { __usageAxisPaint: WeakMap<HTMLCanvasElement, Frame> };

/** Record native draws and real TextMetrics; the original canvas methods still paint every pixel. */
async function observeCanvas(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const frames = new WeakMap<HTMLCanvasElement, Frame>();
    Object.defineProperty(window, '__usageAxisPaint', { value: frames });
    const clear: unknown = Reflect.get(CanvasRenderingContext2D.prototype, 'clearRect');
    if (typeof clear !== 'function') throw new Error('The native canvas clear method is missing');
    CanvasRenderingContext2D.prototype.clearRect = function (x, y, width, height) {
      Reflect.apply(clear, this, [x, y, width, height]);
      if (this.canvas instanceof HTMLCanvasElement && x === 0 && y === 0 && width >= this.canvas.width && height >= this.canvas.height) {
        frames.set(this.canvas, { number: (frames.get(this.canvas)?.number ?? 0) + 1, labels: [] });
      }
    };
    const draw: unknown = Reflect.get(CanvasRenderingContext2D.prototype, 'fillText');
    if (typeof draw !== 'function') throw new Error('The native canvas text method is missing');
    CanvasRenderingContext2D.prototype.fillText = function (text, x, y, maxWidth) {
      Reflect.apply(draw, this, maxWidth === undefined ? [text, x, y] : [text, x, y, maxWidth]);
      if (!(this.canvas instanceof HTMLCanvasElement)) return;
      const frame = frames.get(this.canvas);
      if (frame === undefined) return;
      const metrics = this.measureText(text);
      const transform = this.getTransform();
      const point = (left: number, top: number) => ({ x: transform.a * left + transform.c * top + transform.e, y: transform.b * left + transform.d * top + transform.f });
      const corners = [point(x - metrics.actualBoundingBoxLeft, y - metrics.actualBoundingBoxAscent), point(x + metrics.actualBoundingBoxRight, y - metrics.actualBoundingBoxAscent),
        point(x - metrics.actualBoundingBoxLeft, y + metrics.actualBoundingBoxDescent), point(x + metrics.actualBoundingBoxRight, y + metrics.actualBoundingBoxDescent)];
      const anchor = point(x, y);
      frame.labels.push({ text, x: anchor.x, y: anchor.y, left: Math.min(...corners.map((at) => at.x)), right: Math.max(...corners.map((at) => at.x)),
        top: Math.min(...corners.map((at) => at.y)), bottom: Math.max(...corners.map((at) => at.y)), font: this.font,
        fill: typeof this.fillStyle === 'string' ? this.fillStyle : this.fillStyle instanceof CanvasGradient ? 'gradient' : 'pattern',
        align: this.textAlign, baseline: this.textBaseline });
    };
  });
}

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

async function painted(canvas: Locator) {
  return canvas.evaluate((element: HTMLCanvasElement) => {
    const frame = (window as ObservedWindow).__usageAxisPaint.get(element);
    const ctx = element.getContext('2d');
    const host = element.closest('[role="group"]');
    const plot = element.parentElement?.querySelector('.u-over');
    const normal = document.createElement('canvas').getContext('2d');
    if (frame === undefined || ctx === null || host === null || plot === null || plot === undefined || normal === null) throw new Error('The production chart has no observed canvas frame');
    const rect = element.getBoundingClientRect(), plotRect = plot.getBoundingClientRect();
    const css = getComputedStyle(host);
    // Canonicalize the unchanged product font/ink through the browser's own CSS parser.
    normal.font = `${Math.round(12 * devicePixelRatio)}px ${css.fontFamily}`;
    normal.fillStyle = css.getPropertyValue('--text-muted').trim();
    const labels = frame.labels.map((label) => {
      const left = Math.max(0, Math.floor(label.left)), top = Math.max(0, Math.floor(label.top));
      const right = Math.min(element.width, Math.ceil(label.right)), bottom = Math.min(element.height, Math.ceil(label.bottom));
      const pixels = right > left && bottom > top ? ctx.getImageData(left, top, right - left, bottom - top).data : new Uint8ClampedArray();
      let ink = 0;
      for (let at = 3; at < pixels.length; at += 4) if ((pixels[at] ?? 0) > 0) ink += 1;
      return { ...label, ink };
    });
    return { frame: frame.number, labels, expectedFont: normal.font, expectedInk: normal.fillStyle, dpr: devicePixelRatio,
      width: element.width, height: element.height, cssWidth: rect.width, cssHeight: rect.height, hostWidth: host.clientWidth,
      plot: { left: (plotRect.left - rect.left) * devicePixelRatio, right: (plotRect.right - rect.left) * devicePixelRatio,
        top: (plotRect.top - rect.top) * devicePixelRatio, bottom: (plotRect.bottom - rect.top) * devicePixelRatio } };
  });
}

function tick(locale: string, bucket: 'day' | 'hour', stamp: number): string {
  return new Intl.DateTimeFormat(locale, { ...(bucket === 'hour' ? { hour: 'numeric', minute: '2-digit' } as const : { month: 'short', day: 'numeric' } as const), timeZone: 'UTC' }).format(new Date(stamp));
}

function reading(locale: string, bucket: 'day' | 'hour', stamp: string): string {
  return new Intl.DateTimeFormat(locale, { ...(bucket === 'hour' ? { dateStyle: 'medium', timeStyle: 'short' } as const
    : { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' } as const), timeZone: 'UTC' }).format(new Date(stamp));
}

// Every locale/theme pair is covered; each language and theme is also observed at DPR 1 and 2.
const modes = [
  { locale: 'en', theme: 'light', dpr: 1 }, { locale: 'en', theme: 'dark', dpr: 2 },
  { locale: 'zh-CN', theme: 'light', dpr: 2 }, { locale: 'zh-CN', theme: 'dark', dpr: 1 },
] as const;

for (const mode of modes) test.describe(`${mode.locale}/${mode.theme}/DPR ${mode.dpr}`, () => {
  test.use({ deviceScaleFactor: mode.dpr, viewport: { width: 1440, height: 900 } });

  test('Usage axis ink fits after font loading, daily/hourly intervals and narrow resizing', async ({ page }, info) => {
    await scenario(); await observeCanvas(page); await page.emulateMedia({ colorScheme: mode.theme }); await signIn(page);
    const words = mode.locale === 'en' ? english : chinese;
    if (mode.locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const calls: { method: string; path: string; query: string }[] = [];
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (url.pathname.startsWith('/api/v1/usage')) calls.push({ method: request.method(), path: url.pathname, query: url.search });
    });
    const answer = (bucket: 'day' | 'hour', day?: string) => page.waitForResponse((response) => {
      const url = new URL(response.url());
      return response.request().method() === 'GET' && url.pathname === '/api/v1/usage' && url.searchParams.get('scope') === 'me'
        && url.searchParams.get('bucket') === bucket && (day === undefined || url.searchParams.get('from') === day && url.searchParams.get('to') === day);
    });
    let releaseFont: () => void = () => undefined;
    const fontGate = new Promise<void>((resolve) => { releaseFont = resolve; });
    let heldFonts = 0;
    const delayFont = mode.locale === 'en' && mode.theme === 'light';
    if (delayFont) await page.route(/\/inter-latin-wght-normal[^/]*\.woff2(?:\?.*)?$/, async (route) => {
      heldFonts += 1; await fontGate; await route.continue();
    });
    try {
      const loaded = answer('day');
      // DOM readiness permits the real font-display:swap frame while the actual font response is held.
      await page.goto('/usage?range=7d&bucket=day', { waitUntil: 'domcontentloaded' });
      const response = await loaded; expect(response.status()).toBe(200);
      let report = await response.json() as UsageAnalytics;
      expect(report.bucket).toBe('day'); expect(report.series).toHaveLength(7);
      const span = { from: report.from, to: report.to };
      await expect(page.locator('html')).toHaveAttribute('lang', mode.locale);
      await expect(page.locator('html')).toHaveAttribute('data-theme', mode.theme);
      // A wide browser can still hold a narrow main pane. Use the existing
      // Focus mode so the wide case actually exercises the final endpoint tick.
      await page.getByRole('button', { name: words.actions.layout.label, exact: true }).click();
      await page.getByRole('menuitemcheckbox', { name: words.workbench.workspace.focus, exact: true }).click();
      await expect(page.locator('[data-workbench-chrome]')).toHaveCount(0);
      await expect(page.locator('[data-workbench-bar]')).toHaveCount(0);
      const section = page.getByRole('region', { name: words.usage.sections.overTime, exact: true });
      const group = section.getByRole('group', { name: `${words.usage.chart.label}. ${words.usage.chart.keys}`, exact: true });
      const canvas = group.locator('canvas');
      await expect(canvas).toBeVisible();
      await expect.poll(async () => (await painted(canvas)).labels.length).toBeGreaterThan(0);
      if (delayFont) {
        await expect.poll(() => heldFonts).toBeGreaterThan(0);
        expect(await page.evaluate(() => document.fonts.status)).toBe('loading');
        const before = (await painted(canvas)).frame;
        releaseFont(); await settled(page);
        await expect.poll(async () => (await painted(canvas)).frame).toBeGreaterThan(before);
        await info.attach('usage-axis-font-swap', { body: JSON.stringify({ heldFonts, beforeFrame: before, afterFrame: (await painted(canvas)).frame,
          fontStatus: await page.evaluate(() => document.fonts.status) }), contentType: 'application/json' });
      } else releaseFont();

      for (const bucket of ['day', 'hour'] as const) {
        if (bucket === 'hour') {
          const changed = answer('hour');
          await section.getByRole('combobox', { name: words.usage.chart.per, exact: true }).click();
          await page.getByRole('option', { name: words.usagePerf.choices.buckets.hour.name, exact: true }).click();
          const response = await changed; expect(response.status()).toBe(200);
          report = await response.json() as UsageAnalytics;
          expect(report.bucket).toBe('hour'); expect(report.series).toHaveLength(7 * 24);
          expect({ from: report.from, to: report.to }).toEqual(span);
          const query = new URL(response.url()).searchParams;
          expect(query.get('scope')).toBe('me'); expect(query.get('from')).toBe(span.from); expect(query.get('to')).toBe(span.to);
          await expect(page).toHaveURL(/bucket=hour/);
        }
        await expect(section.getByRole('combobox', { name: words.usage.chart.per, exact: true })).toHaveText(words.usagePerf.choices.buckets[bucket].name);
        await expect(section.locator('figure')).toHaveAttribute('aria-busy', 'false');
        const first = report.series[0]?.at, last = report.series.at(-1)?.at;
        if (first === undefined || last === undefined) throw new Error('The real usage fixture has no chart buckets');
        const end = Date.parse(last) + (bucket === 'day' ? 86_400_000 : 3_600_000);
        const tickWords = new Set([...report.series.map((point) => tick(mode.locale, bucket, Date.parse(point.at))), tick(mode.locale, bucket, end)]);
        for (const width of [1440, 400]) {
          await settled(page);
          const beforeResize = calls.slice();
          await page.setViewportSize({ width, height: 900 });
          await section.scrollIntoViewIfNeeded(); await page.mouse.move(0, 0); await settled(page);
          await expect.poll(() => canvas.evaluate((element) => Math.abs(element.getBoundingClientRect().width - (element.closest('[role="group"]')?.clientWidth ?? 0)))).toBeLessThanOrEqual(1);
          const drawing = await painted(canvas);
          expect(drawing.dpr).toBe(mode.dpr);
          expect(Math.abs(drawing.cssWidth - drawing.hostWidth)).toBeLessThanOrEqual(1);
          expect(Math.abs(drawing.width - drawing.cssWidth * mode.dpr)).toBeLessThanOrEqual(1);
          expect(drawing.cssHeight).toBe(200);
          const times = drawing.labels.filter((label) => label.align === 'center' && label.baseline === 'top');
          const values = drawing.labels.filter((label) => label.align === 'right' && label.baseline === 'middle');
          expect(times.length).toBeGreaterThanOrEqual(2); expect(values.length).toBeGreaterThanOrEqual(2);
          for (const label of [...times, ...values]) {
            const reason = `${bucket}/${width}/DPR ${mode.dpr}: ${label.text}`;
            // One backing pixel allows native glyph rasterization rounding, not a clipped character.
            expect(label.left, reason).toBeGreaterThanOrEqual(-1); expect(label.right, reason).toBeLessThanOrEqual(drawing.width + 1);
            expect(label.top, reason).toBeGreaterThanOrEqual(-1); expect(label.bottom, reason).toBeLessThanOrEqual(drawing.height + 1);
            expect(label.ink, reason).toBeGreaterThan(0);
            expect(label.font, reason).toBe(drawing.expectedFont); expect(label.fill, reason).toBe(drawing.expectedInk);
          }
          for (const label of times) {
            expect(tickWords.has(label.text), label.text).toBe(true);
            expect(label.y).toBeGreaterThanOrEqual(drawing.plot.bottom);
          }
          if (width === 1440) {
            const final = times.at(-1);
            expect(final?.text).toBe(tick(mode.locale, bucket, end));
            expect(Math.abs((final?.x ?? Number.NaN) - drawing.plot.right)).toBeLessThanOrEqual(1);
          }
          await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
          expect(calls).toEqual(beforeResize);

          const beforeReading = calls.slice(), address = page.url();
          await group.focus(); await group.press('Home');
          await expect(section.getByRole('status')).toContainText(reading(mode.locale, bucket, first));
          await group.press('End');
          await expect(section.getByRole('status')).toContainText(reading(mode.locale, bucket, last));
          await group.press('Tab');
          await section.getByRole('button', { name: words.usage.chart.table, exact: true }).click();
          const table = section.getByRole('table', { name: `${words.usage.chart.label}: ${words.usage.chart.table}`, exact: true });
          await expect(table.getByRole('row')).toHaveCount(report.series.length + 1);
          await expect(table.getByRole('row').nth(1)).toContainText(reading(mode.locale, bucket, first));
          await section.getByRole('button', { name: words.usage.chart.table, exact: true }).click();
          expect(page.url()).toBe(address); expect(calls).toEqual(beforeReading);
          await section.scrollIntoViewIfNeeded(); await page.mouse.move(0, 0); await settled(page);
          const retained = await painted(canvas);
          expect(retained.width).toBe(drawing.width); expect(retained.labels).toEqual(drawing.labels);
          const stem = `usage-axis-${mode.locale}-${mode.theme}-dpr${mode.dpr}-${bucket}-${width}`;
          const path = info.outputPath(`${stem}.png`);
          await section.screenshot({ path, caret: 'hide' });
          await info.attach(stem, { path, contentType: 'image/png' });
          await info.attach(`${stem}-paint`, { body: JSON.stringify(retained, null, 2), contentType: 'application/json' });
        }
      }
      // Enter retains the native day-range contract even when the chosen bar is hourly.
      const lastDay = report.series.at(-1)?.at.slice(0, 10);
      if (lastDay === undefined) throw new Error('No last hour to select');
      const selected = answer('hour', lastDay);
      await group.focus(); await group.press('End'); await group.press('Enter');
      const responseAfterSelection = await selected; expect(responseAfterSelection.status()).toBe(200);
      const narrowed = await responseAfterSelection.json() as UsageAnalytics;
      expect(narrowed.from).toBe(lastDay); expect(narrowed.to).toBe(lastDay); expect(narrowed.bucket).toBe('hour');
      await expect.poll(() => new URL(page.url()).searchParams.get('from')).toBe(lastDay);
      await expect.poll(() => new URL(page.url()).searchParams.get('to')).toBe(lastDay);
      expect(calls.every((call) => call.method === 'GET')).toBe(true);
    } finally {
      releaseFont();
      await info.attach('usage-axis-native-calls', { body: JSON.stringify(calls, null, 2), contentType: 'application/json' });
    }
  });
});
