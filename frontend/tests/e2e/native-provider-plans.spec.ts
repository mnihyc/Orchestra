/** The existing provider card shows declared source plan names instead of native wire ids. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { FACT_MEMBERS } from '../../src/features/resources/providers/details';
import type { LibraryEntry, Provider } from '../../src/generated/api-types';
import { scenario, signIn } from './support';

const PLANS = [
  ['pro', 'Pro (More)'], ['prolite', 'Pro'], ['promax', 'Pro (Max)'], ['ent26', 'Enterprise'],
  ['enterprise_cbp_automation', 'Enterprise (Automation)'],
  ['enterprise_cbp_usage_based', 'Enterprise CBP Usage Based'],
  ['sci', 'Academic Researchers'], ['sci_plus', 'Academic Researchers Plus'],
] as const;

async function seedProviderFacts(provider: Provider, onRefresh = false): Promise<void> {
  const double = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
  const response = await fetch(`${double}/__double/resources/provider-facts`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      provider_id: provider.id, name: provider.name, facts: provider.facts,
      facts_state: provider.facts_state, sources: provider.sources, on_refresh: onRefresh,
    }),
  });
  expect(response.ok).toBe(true);
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`a provider refresh retains its declared account plan ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const id = RUNS_ON.codexSubscription;
    const provider = planProvider({ id, name: 'Illustration account', harness_kind: 'codex' });
    const source = provider.sources.facts.descriptor;
    if (source?.facts === null || source?.facts === undefined) throw new Error('The native source fixture must declare its plan names');
    source.title = 'Codex subscription';
    source.facts.plans = Object.fromEntries(PLANS);
    provider.facts.plan = 'pro';
    await seedProviderFacts(provider);
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${id}?tab=overview`);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const card = page.locator('[data-provider-usage="full"]');
    await expect(card.locator('[data-fact-member="plan"]')).toContainText('Pro (More)');
    for (const [plan, label] of PLANS) {
      provider.facts.plan = plan;
      await seedProviderFacts(provider, true);
      await card.getByRole('button', { name: words.providers.overview.refresh, exact: true }).click();
      const line = card.locator('[data-fact-member="plan"]');
      await expect(line.getByText(label, { exact: true })).toBeVisible();
      await expect(line.getByText(plan, { exact: true })).toHaveCount(0);
    }
    await page.reload();
    await expect(card.locator('[data-fact-member="plan"]')).toContainText('Academic Researchers Plus');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('native-plan-names', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });

  test(`unavailable provider facts keep their source and usage entrance visible ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const id = RUNS_ON.codexSubscription;
    const provider = planProvider({ id, name: 'Illustration account', harness_kind: 'codex' });
    const source = provider.sources.facts.descriptor;
    if (source === null) throw new Error('The native source fixture must have a declaration');
    source.title = 'Declared facts source';
    provider.facts_state.plan = 'not_read_yet';
    provider.facts.plan = 'Earlier account plan';
    provider.facts_state.balances = 'unsupported';
    const path = `/api/v1/providers/${id}`;
    let historyReads = 0;
    page.on('request', (request) => { if (new URL(request.url()).pathname === `${path}/plan`) historyReads += 1; });
    await seedProviderFacts(provider);
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${id}?tab=overview`);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const card = page.locator('[data-provider-usage="full"]');
    const plan = card.locator('[role="group"][data-fact-member="plan"]');
    await expect(plan).toContainText(words.providerDetails.notReadOnce);
    await expect(card.getByRole('heading', { name: words.providerDetails.title.replace('{{source}}', source.title), exact: true })).toBeVisible();
    await expect(plan).not.toContainText(source.title);
    await expect(plan).not.toContainText('Earlier account plan');
    const missing = card.locator('[data-unsupported-members]');
    await expect(missing).toHaveCount(1);
    await expect(missing).toContainText(words.providerDetails.members.balances);
    await expect(missing).not.toContainText(source.title);
    await expect(missing).not.toContainText('12.4');
    await expect(card.locator('[role="group"][data-fact-member="balances"]')).toHaveCount(0);
    provider.facts_state.plan = 'told'; provider.facts.plan = 'Pro';
    await seedProviderFacts(provider, true);
    await card.getByRole('button', { name: words.providers.overview.refresh, exact: true }).click();
    await expect(plan.getByText('Pro', { exact: true })).toBeVisible();
    for (const member of ['windows', 'balances', 'spend', 'extra_usage'] as const) provider.facts_state[member] = 'unsupported';
    await seedProviderFacts(provider, true);
    await card.getByRole('button', { name: words.providers.overview.refresh, exact: true }).click();
    await expect(missing).toContainText(words.providerDetails.members.windows);
    await expect(card.locator('[role="group"][data-fact-member="windows"]')).toHaveCount(0);
    await page.getByRole('tab', { name: words.providers.tabs.usage, exact: true }).click();
    await expect(page.getByText(words.providerCredits.historyUnsupported, { exact: true })).toBeVisible();
    expect(historyReads).toBe(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('unavailable-provider-facts', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });

  test(`provider overview groups the keeper's missing facts and names its turn counts ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    const id = RUNS_ON.codexSubscription;
    const provider = planProvider({ id, name: 'Credential keeper account' });
    provider.sources.facts = { ...provider.sources.facts, by: 'none', name: null, descriptor: null };
    for (const member of FACT_MEMBERS) provider.facts_state[member] = member === 'credential' ? 'told' : 'unsupported';
    await seedProviderFacts(provider);
    const entry: LibraryEntry = {
      target: { object: { kind: 'provider', id }, version_id: null }, threads: 1,
      facts: { subject: { kind: 'resource', object: { kind: 'provider', id } }, as_of: provider.updated_at, dependents_complete: true,
        dependents: [{ relation: 'binding', count: 1, hidden: 0, until: null, visible: [{ object: null, name: 'Visible conversation', relation: 'binding', until: null }] }],
        uses: { kind: 'turn', all_time: 1, last_30_days: 1, people: 1, people_30_days: 1, last_used_at: null, coverage: { state: 'complete' } } },
    };
    await page.route('**/api/v1/resources/library', async (route) => { await route.fulfill({ json: [entry] }); });
    await signIn(page);
    await page.goto('/settings/interface');
    await page.getByRole('radio', { name: theme === 'dark' ? 'Dark' : 'Light', exact: true }).click();
    if (locale === 'zh-CN') {
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${id}?tab=overview`);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const card = page.locator('[data-provider-usage="full"]');
    await expect(card.getByRole('heading', { name: words.providerDetails.title.replace('{{source}}', words.providerDetails.keeperSource), exact: true })).toBeVisible();
    await expect(card.getByRole('group')).toHaveCount(1);
    await expect(card.locator('[data-fact-member="credential"]')).toContainText(words.providerDetails.tokenShort);
    const names = new Intl.ListFormat(locale, { type: 'unit' }).format(FACT_MEMBERS.filter((member) => member !== 'credential').map((member) => words.providerDetails.members[member]));
    const summary = card.locator('[data-unsupported-members]');
    await expect(summary).toHaveCount(1);
    await expect(summary).toHaveText(words.providerDetails.notSuppliedMembers.replace('{{members}}', names));
    await expect(summary).not.toContainText(words.providerDetails.keeperSource);
    await expect(card.getByRole('meter')).toHaveCount(0);
    const counts = page.locator('[data-library-counts]').first();
    const lifetime = locale === 'en' ? '1 turn all time' : '累计 1 轮';
    const recent = locale === 'en' ? '1 turn in the last 30 days' : '最近 30 天 1 轮';
    await expect(counts.getByText(lifetime, { exact: true })).toBeVisible();
    await expect(counts.getByText(recent, { exact: true })).toBeVisible();
    await expect(counts.getByText(locale === 'en' ? 'Given 1 thread' : '已提供给 1 个会话', { exact: true })).toBeVisible();
    await expect(counts.getByText(locale === 'en' ? 'Used 1 turn' : '已使用 1 轮', { exact: true })).toBeVisible();
    await page.mouse.move(width - 1, 899);
    await counts.getByRole('button', { name: words.library.usedTitle, exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByText(lifetime, { exact: true })).toHaveCount(2);
    await expect(page.getByText(recent, { exact: true })).toHaveCount(2);
    await page.keyboard.press('Escape');
    await expect(page.getByText(lifetime, { exact: true })).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await summary.scrollIntoViewIfNeeded();
    const bounds = await summary.boundingBox();
    expect(bounds).not.toBeNull();
    expect(bounds?.x).toBeGreaterThanOrEqual(0);
    expect((bounds?.x ?? 0) + (bounds?.width ?? 0)).toBeLessThanOrEqual(width + 1);
    await info.attach('provider-overview-facts-and-turns', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
