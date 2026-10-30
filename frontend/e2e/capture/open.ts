/** Open the application's real controls, using catalogue keys for built-in states in either locale. */
import { expect, type Locator, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { ACTION_MS, type CaptureLocale, type ClickTarget } from './config.ts';
import type { CaptureState } from './manifest.ts';

const catalogues = new Map<CaptureLocale, unknown>();
export function caption(name: string, locale: CaptureLocale): string {
  if (!name.startsWith('@')) return name;
  let catalogue = catalogues.get(locale);
  if (catalogue === undefined) {
    catalogue = JSON.parse(readFileSync(new URL(`../../src/i18n/${locale}.json`, import.meta.url), 'utf8')) as unknown;
    catalogues.set(locale, catalogue);
  }
  let value: unknown = catalogue;
  for (const key of name.slice(1).split('.')) value = typeof value === 'object' && value !== null && key in value ? (value as Record<string, unknown>)[key] : undefined;
  if (typeof value !== 'string') throw new Error('A capture control has no catalogue entry.');
  return value;
}
const escapePattern = (value: string) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
export function target(page: Page, click: ClickTarget, locale: CaptureLocale): Locator {
  const scope = click.within === undefined ? page : page.locator(click.within);
  const name = caption(click.name, locale);
  return scope.getByRole(click.role, { name: click.startsWith === true ? new RegExp(`^${escapePattern(name)}`) : name, exact: click.startsWith !== true });
}

async function container(page: Page, id: string, locale: CaptureLocale): Promise<void> {
  const bar = page.locator('[data-workbench-bar="right"]:visible');
  const name = caption(`@workbench.containers.${id}.title`, locale);
  if ((page.viewportSize()?.width ?? 1440) < 900) {
    const choice = page.locator(`[data-container-choice="right:${id}"]`);
    await choice.click({ timeout: ACTION_MS });
    await expect(choice).toHaveAttribute('aria-pressed', 'true', { timeout: ACTION_MS });
    await expect(page.getByRole('dialog', { name, exact: true })).toBeVisible({ timeout: ACTION_MS });
  } else {
    const tab = page.locator('[data-workbench-bar="right"]').getByRole('tab', { name, exact: true });
    await tab.click({ timeout: ACTION_MS });
    await expect(tab).toHaveAttribute('aria-selected', 'true', { timeout: ACTION_MS });
  }
  await expect(bar).toBeVisible({ timeout: ACTION_MS });
}

async function chooser(page: Page, locale: CaptureLocale): Promise<Locator> {
  let add = page.getByRole('button', { name: caption('@workbench.group.add', locale), exact: true }).filter({ visible: true }).first();
  if (!await add.isVisible() && (page.viewportSize()?.width ?? 1440) < 900) {
    await container(page, 'conversation', locale);
    add = page.getByRole('button', { name: caption('@workbench.group.add', locale), exact: true }).filter({ visible: true }).first();
  }
  await add.click({ timeout: ACTION_MS });
  const dialog = page.getByRole('dialog', { name: caption('@workbench.openView.choose', locale), exact: true });
  await expect(dialog).toBeVisible({ timeout: ACTION_MS });
  return dialog;
}

export async function openState(page: Page, state: CaptureState, locale: CaptureLocale): Promise<void> {
  if (state.container !== undefined) await container(page, state.container, locale);
  if (state.view !== undefined) {
    const dialog = await chooser(page, locale);
    if (state.view !== 'chooser') {
      await dialog.locator(`[role="option"][data-value="commands:view.open.${state.view}"]`).click({ timeout: ACTION_MS });
      await expect(dialog).not.toBeVisible({ timeout: ACTION_MS });
      // A chooser disappearing is insufficient: require the requested body to have actually opened.
      const body = page.locator(`[data-section-view="${state.view}"] > div[id^="section-body-"]:visible`).or(page.locator('[data-revealed-view]:visible'));
      await expect((state.view === 'terminal' ? body.or(page.getByRole('tabpanel', { name: caption('@views.terminal.title', locale), exact: true })) : body).first()).toBeVisible({ timeout: ACTION_MS });
    }
  }
  for (const click of state.clicks ?? []) await target(page, click, locale).click({ timeout: ACTION_MS });
  if (state.dialog !== undefined) {
    const dialog = state.dialog === '*' ? page.getByRole('dialog').filter({ visible: true }).first()
      : page.getByRole('dialog', { name: caption(state.dialog, locale), exact: true });
    await expect(dialog).toBeVisible({ timeout: ACTION_MS });
  }
}
