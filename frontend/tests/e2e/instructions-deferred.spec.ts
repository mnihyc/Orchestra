/** A resumed session explains its retained instructions in an ordinary history line. */
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { agentMessage, marker, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { begin, MAIN } from './inspector-support';
import { emit, PEOPLE, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const SCREEN_DIR = resolve(process.env.ORCH_INSTRUCTIONS_SCREEN_DIR ?? 'output/playwright/instructions');
for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`retained instructions stay informational after another turn and reload at ${width}px in ${theme}/${locale}`, async ({ page, request }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await begin('');
    const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: PEOPLE.ada });
    expect(login.ok()).toBe(true);
    const { access_token: token } = await login.json() as { access_token: string };
    const saved = await request.patch(`${DOUBLE}/api/v1/me`, { headers: { Authorization: `Bearer ${token}` }, data: { locale, theme } });
    expect(saved.ok()).toBe(true);
    const words = locale === 'en' ? english : chinese;
    const sentence = words.timeline.marker.instructionsDeferred.replace('{{program}}', 'Codex');
    await emit(MAIN, {
      turns: [turn('resumed-turn', { ordinal: 1, state: 'completed', inputId: 'resumed-input' })],
      items: [
        userMessage('resumed-input', 'Continue the work', { author: ADA, turn: 'resumed-turn' }),
        marker('retained-instructions', { marker: 'instructions_deferred', data: { program: 'Codex' } }, { turn: 'resumed-turn' }),
        agentMessage('resumed-answer', 'The checks passed', { turn: 'resumed-turn' }),
      ],
    });
    await signIn(page, PEOPLE.ada);
    await page.goto(`/thread/${MAIN}`);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const notice = page.getByText(sentence, { exact: true });
    await expect(notice).toBeVisible();
    await expect(notice.locator('..')).toHaveClass(/text-text-subtle/);
    await expect(notice.locator('..').getByRole('button')).toHaveCount(0);
    await expect(page.getByText('The checks passed', { exact: true })).toBeVisible();
    await mkdir(SCREEN_DIR, { recursive: true });
    await page.screenshot({ path: resolve(SCREEN_DIR, `${width}-${theme}-${locale}-resumed.png`) });
    await emit(MAIN, {
      turns: [turn('next-turn', { ordinal: 2, state: 'completed', inputId: 'next-input' })],
      items: [
        userMessage('next-input', 'Continue once more', { author: ADA, turn: 'next-turn' }),
        agentMessage('next-answer', 'The follow-up also passed', { turn: 'next-turn' }),
      ],
    });
    await expect(page.getByText('The follow-up also passed', { exact: true })).toBeVisible();
    await expect(notice).toHaveCount(1);
    await page.reload();
    await expect(notice).toHaveCount(1);
    await expect(notice).toBeVisible();
    await expect(notice.locator('..')).toHaveClass(/text-text-subtle/);
    await expect(page.getByText('The follow-up also passed', { exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: resolve(SCREEN_DIR, `${width}-${theme}-${locale}-reloaded.png`) });
  });
}
