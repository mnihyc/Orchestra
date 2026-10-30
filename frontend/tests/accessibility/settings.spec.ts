/** Every page of settings, asked in both themes whether anything on it is a serious barrier: a person's own pages, each section of an org's, a project's and a topic's, with what opens above them open, for somebody who may change everything and for somebody who may only read. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, expectAccessibleSettled, PEOPLE, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

test('a person’s own pages: profile, security with its dialogs, tokens and the interface', async ({ page }) => {
  await open(page, '/settings/profile');
  await expect(page.getByRole('heading', { name: 'Profile', level: 1 })).toBeVisible();
  await expectAccessibleSettled(page);

  await page.getByRole('button', { name: 'Change address…' }).click();
  await expect(page.getByRole('dialog', { name: 'Change your address' })).toBeVisible();
  await expectAccessibleSettled(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.goto('/settings/security');
  await expect(page.getByRole('switch', { name: 'Authenticator app' })).toBeVisible();
  await expectAccessibleSettled(page);
  await page.getByRole('switch', { name: 'Authenticator app' }).click();
  await page.getByRole('dialog', { name: 'Set up an authenticator app' }).getByLabel('Your password', { exact: true }).fill(PEOPLE.ada.password);
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page.getByRole('img', { name: 'Code to scan with your authenticator app' })).toBeVisible();
  await expectAccessibleSettled(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.goto('/settings/tokens');
  await expect(page.getByText('You have no API tokens.')).toBeVisible();
  await page.getByRole('button', { name: 'New token…' }).click();
  await page.getByRole('radio', { name: 'Selected', exact: true }).click();
  await expect(page.getByRole('checkbox', { name: /View Thread/ })).toBeVisible();
  await expectAccessibleSettled(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.goto('/settings/interface');
  await expect(page.getByRole('region', { name: 'On this device' })).toBeVisible();
  await expectAccessibleSettled(page);
});

/** The sections of an org's settings, a few to a test: asked in both themes each takes a while, and a test of all of them together outlasts its time on a busy machine. */
const ORG_SECTIONS = [
  ['general', 'departments', 'members'],
  ['groups', 'roles', 'policies'],
  ['tags', 'service-accounts'],
  ['audit', 'channels', 'destinations'],
] as const;
const SECTION_NAMES: Readonly<Record<string, string>> = { general: 'General', departments: 'Departments', members: 'Members', groups: 'Groups',
  roles: 'Roles and access', policies: 'Defaults', tags: 'Tags', 'service-accounts': 'Service accounts', audit: 'Audit', channels: 'Outside Orch', destinations: 'Outside Orch' };

for (const sections of ORG_SECTIONS) {
  test(`an org’s sections ${sections.join(', ')}, as the person who makes its rules`, async ({ page }) => {
    await open(page, `/org/acme/settings/${sections[0]}`);
    for (const section of sections) {
      await page.goto(`/org/acme/settings/${section}`);
      await expect(page.getByRole('heading', { name: SECTION_NAMES[section], level: 1 })).toBeVisible();
      await expect(page.getByRole('navigation', { name: 'Sections of the settings of Acme' }).locator('a[aria-current="page"]')).toBeVisible();
      await expectAccessibleSettled(page);
    }
  });
}

test('the dialogs of an org’s settings: a member moved, held out and removed, a group’s people, a tag, a channel', async ({ page }) => {
  await open(page, '/org/acme/settings/members');
  await page.getByRole('button', { name: 'Actions for Grace Hopper' }).click();
  await expectAccessibleSettled(page, '[role="menu"]');
  await page.getByRole('menuitem', { name: 'Suspend…' }).click();
  await expectAccessibleSettled(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Actions for Dora Developer' }).click();
  await page.getByRole('menuitem', { name: 'Remove from Acme…' }).click();
  await expect(page.getByText('Dora’s API key')).toBeVisible();
  await expectAccessibleSettled(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.goto('/org/acme/settings/groups');
  await page.getByRole('button', { name: 'The people in Backend team' }).click();
  await expect(page.getByRole('dialog', { name: 'People in Backend team' })).toBeVisible();
  await expectAccessibleSettled(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.goto('/org/acme/settings/channels');
  await page.getByRole('button', { name: 'Edit Ops room' }).click();
  await expect(page.getByRole('dialog', { name: 'Edit Ops room' })).toBeVisible();
  await expectAccessibleSettled(page, '[role="dialog"]');
});

test('a department’s sections and the settings editor with a lock above', async ({ page }) => {
  await open(page, '/org/acme.research/settings/policies');
  await expect(page.locator('[data-setting="sandbox.mode"] [data-provenance="locked"]')).toBeVisible();
  await expectAccessibleSettled(page);

  for (const section of ['general', 'tags']) {
    await page.goto(`/org/acme.research/settings/${section}`);
    await expect(page.getByRole('heading', { name: SECTION_NAMES[section], level: 1 })).toBeVisible();
    await expectAccessibleSettled(page);
  }
  await page.goto('/org/acme.research/settings/channels');
  await expect(page.getByText('Set for all of Acme.')).toBeVisible();
  await expectAccessibleSettled(page);
});

test('a project’s settings, section by section, and a topic’s', async ({ page }) => {
  await open(page, `/project/${ACME.web}/settings`);
  for (const section of ['', '?section=policies', '?section=tags', '?section=destinations']) {
    await page.goto(`/project/${ACME.web}/settings${section}`);
    const current = section.includes('policies') ? 'Defaults' : section.includes('tags') ? 'Tags' : section.includes('destinations') ? 'Outside Orch' : 'General';
    await expect(page.getByRole('heading', { name: current, level: 1 })).toBeVisible();
    await expectAccessibleSettled(page);
  }
  for (const section of ['', '&section=policies', '&section=destinations']) {
    await page.goto(`/topic/${ACME.fixLogin}?tab=settings${section}`);
    await expect(page.getByRole('navigation', { name: 'Sections of the settings of Fix login' })).toBeVisible();
    await expectAccessibleSettled(page);
  }
});

for (const sections of [['general', 'members', 'policies'], ['tags', 'channels', 'destinations']]) {
  test(`the pages ${sections.join(', ')} for somebody who may only read them`, async ({ page }) => {
    await open(page, '/org/acme/settings/general', PEOPLE.vera);
    for (const section of sections) {
      await page.goto(`/org/acme/settings/${section}`);
      await expect(page.getByRole('heading', { name: SECTION_NAMES[section], level: 1 })).toBeVisible();
      await expectAccessibleSettled(page);
    }
  });
}

test('a project’s policies for somebody who may only read them', async ({ page }) => {
  await open(page, `/project/${ACME.web}/settings?section=policies`, PEOPLE.vera);
  await expect(page.getByRole('heading', { name: 'Defaults', level: 1 })).toBeVisible();
  await expectAccessibleSettled(page);
});
