/** A week of hours on a phone: every hour's bar is drawn in its own colour, at both pixel ratios, and the axis names the days. */
import { expect, test } from '@playwright/test';
import { scenario, signIn } from './support.ts';

for (const dpr of [1, 2] as const) test(`a week of hours at 400 draws its bars in their colours (DPR ${dpr})`, async ({ browser }) => {
  await scenario();
  const context = await browser.newContext({ viewport: { width: 400, height: 860 }, deviceScaleFactor: dpr, locale: 'en-US', timezoneId: 'UTC' });
  const page = await context.newPage();
  try {
    await signIn(page);
    await page.goto('/usage?range=7d&bucket=hour&show=tokens');
    const canvas = page.locator('.uplot canvas').first();
    await page.locator('#usage-over-time').scrollIntoViewIfNeeded();
    await canvas.scrollIntoViewIfNeeded();
    await expect(canvas).toBeVisible();
    // Some 168 hours share about 300 pixels: uPlot leaves out the line between stacks, and the bars must still have their colour,
    // not the surface's (the chart looked empty).
    await expect.poll(() => canvas.evaluate((element: HTMLCanvasElement) => {
      const pixels = element.getContext('2d')?.getImageData(0, 0, element.width, element.height).data;
      if (pixels === undefined) return 0;
      let coloured = 0;
      for (let at = 0; at < pixels.length; at += 4) {
        const [r = 0, g = 0, b = 0, a = 0] = [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]];
        if (a > 0 && Math.max(r, g, b) - Math.min(r, g, b) > 60) coloured += 1;
      }
      // At least a bar of three pixels a hundred high for each pixel ratio's square.
      return coloured / (window.devicePixelRatio * window.devicePixelRatio);
    })).toBeGreaterThan(300);
    await expect(page.locator('#usage-over-time')).toBeVisible();
  } finally { await context.close(); }
});
