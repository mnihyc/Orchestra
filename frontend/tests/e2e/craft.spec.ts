/** The shared title bar, useful retained views, explicit act wording and canonical signs as a person uses them. */
import { expect, type Page, test } from '@playwright/test';
import { showWorkbenchView } from './workbench-support.ts';

import { ACME, closeThread, emit, playScene, scenario, signIn, mainArea, screenAddress } from './support.ts';
import { chats } from './chat-support.ts';
import { askAccess, askCapability } from './inbox-support.ts';
import { MAIN_ENVIRONMENT, workspace } from './inspector-support.ts';
import { asAdmin, signInForManagement } from './admin-mode-support.ts';

const id = (kind: number, value: number) => `00000000-0000-4000-8${kind.toString(16).padStart(3, '0')}-${value.toString(16).padStart(12, '0')}`;
const path = (page: Page) => page.locator('[data-screen-titlebar] [data-screen-path]');
const bar = (page: Page) => page.locator('[data-workbench-bar="right"]');
const doc = `/doc/${id(7, 1)}`;

test.beforeEach(async () => { await scenario(); });

test('the page tab names the thing once and the title bar holds only its ancestors', async ({ page }) => {
  await signIn(page);
  const cases: [string, string, string?][] = [
    ['/org/acme.research.ml', 'ML'],
    [`/project/${ACME.payments}`, 'Payments'],
    [`/thread/${ACME.mainThread}`, 'Main thread', 'Running Main thread · Fix login'],
    [doc, 'Rollout plan'],
    [`/doc/${id(0x30, 7)}`, 'Fee rules'],
    [`/doc/${id(0x30, 8)}`, 'Fee table findings'],
    ['/org/acme/settings/general', 'General', 'General · Acme'],
    ['/org/acme.research.ml/settings/general', 'General', 'General · ML'],
    [`/project/${ACME.payments}/settings?section=defaults`, 'Defaults', 'Defaults · Payments'],
    [`/topic/${ACME.fixLogin}?tab=settings&section=destinations`, 'Outside Orch', 'Outside Orch · Fix login'],
    ['/org/acme/messages', 'Messages'],
    ['/settings/profile', 'Profile'],
    ['/help/finding-things', 'Finding things'],
    ['/not-a-route', 'Page not found'],
  ];
  for (const [route, leaf, tabName = leaf] of cases) {
    await page.goto(route);
    await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: tabName, exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(path(page).getByRole('link', { name: leaf, exact: true })).toHaveCount(0);
    await expect(path(page).locator('[aria-current="page"]')).toHaveCount(0);
    await expect(path(page).getByRole('button', { name: 'Ancestor places', exact: true })).toHaveCount(0);
    await expect(page.locator('[data-screen-titlebar]')).toHaveCount(1);
  }
});

test('the phone title bar gives the document its full parent back link', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page);
  await page.goto(doc);
  await expect(path(page).getByRole('link', { name: 'Rollout plan', exact: true })).toHaveCount(0);
  await expect(path(page).getByRole('link', { name: 'Fix login', exact: true })).toHaveAttribute('href', `/topic/${ACME.fixLogin}?tab=overview`);
  await expect(path(page).getByRole('link', { name: 'Fix login', exact: true })).toBeVisible();
  await expect(path(page).getByRole('button', { name: 'Ancestor places' })).toHaveCount(0);
});

test('a chosen chat scope and a closed resized bar survive next thread, topic, return, reload and another window', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Chat');
  await page.getByRole('radio', { name: 'Project', exact: true }).click();
  for (const route of [`/thread/${ACME.repro}`, `/topic/${ACME.fixLogin}`, `/thread/${ACME.mainThread}`]) {
    await page.goto(route);
    await expect(page.getByRole('radio', { name: 'Project', exact: true })).toBeChecked();
  }
  await page.reload();
  await expect(page.getByRole('radio', { name: 'Project', exact: true })).toBeChecked();
  const before = await page.getByRole('region', { name: 'Right bar', exact: true }).boundingBox();
  const handles = page.getByRole('separator');
  for (let index = 0; index < await handles.count(); index += 1) {
    const handle = await handles.nth(index).boundingBox();
    if (handle !== null && before !== null && Math.abs(handle.x - before.x) < 12 && handle.height > 200) {
      await page.mouse.move(handle.x + handle.width / 2, handle.y + 50);
      await page.mouse.down();
      await page.mouse.move(handle.x - 100, handle.y + 50, { steps: 5 });
      await page.mouse.up();
      break;
    }
  }
  const resized = await page.getByRole('region', { name: 'Right bar', exact: true }).boundingBox();
  expect(resized?.width).toBeGreaterThan((before?.width ?? 0) + 50);
  await page.getByRole('button', { name: 'Right bar', exact: true }).click();
  // Closed containers retain their views; the visible presentation stays closed across navigation.
  for (const route of [`/thread/${ACME.repro}`, `/topic/${ACME.fixLogin}`, `/thread/${ACME.mainThread}`]) {
    await page.goto(route);
    await expect(bar(page)).not.toBeVisible();
  }
  await page.reload();
  await expect(bar(page)).not.toBeVisible();
  const context = page.context();
  await page.close();
  const second = await context.newPage();
  try {
    await second.goto(`/thread/${ACME.mainThread}`); await expect(bar(second)).not.toBeVisible();
    await second.getByRole('button', { name: 'Right bar', exact: true }).click();
    const restored = await second.getByRole('region', { name: 'Right bar', exact: true }).boundingBox();
    // Border rounding may differ by one pixel; a changed layout width must not.
    expect(Math.abs((restored?.width ?? 0) - (resized?.width ?? 0))).toBeLessThanOrEqual(2);
  }
  finally { await second.close(); }
});

test('notification settings keeps its name and Bell sign while the chosen level stays a separate value', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Chat');
  const opener = page.getByRole('button', { name: 'Notification settings', exact: true });
  // The bell renders this owner's served All state rather than a generic notification-settings glyph.
  await expect(opener.locator('svg.lucide-bell-ring')).toBeVisible();
  await expect(opener).toHaveAccessibleDescription('Default: All');
  await opener.click();
  await expect(page.getByRole('menuitemradio', { name: 'Default (All)', exact: true })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('Escape');
  await expect(opener).toHaveAccessibleName('Notification settings');
});

test('layout, reading selectors and thread setup use explicit names and their canonical signs', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  for (const [name, act] of [['List', 'list'], ['Board', 'board'], ['Activity', 'activity']] as const) {
    await expect(page.getByRole('radio', { name, exact: true }).locator(`[data-action-icon="${act}"]`)).toBeVisible();
  }
  // Project creation uses New topic; the thread starter and its setup remain on the topic overview.
  await page.goto(`/topic/${ACME.fixLogin}?tab=overview`);
  await expect(page.getByRole('button', { name: 'Thread setup', exact: true }).locator('[data-action-icon="threadSetup"]')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start thread', exact: true }).locator('[data-action-icon="send"]')).toBeVisible();
  await page.getByRole('button', { name: 'Layout', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Reset layout', exact: true }).locator('[data-action-icon="resetLayout"]')).toBeVisible();
});

test('all resource rename entrances use the same sign and target-qualified Actions opener', async ({ page }) => {
  await signInForManagement(page);
  for (const [kind, tag, name] of [['providers', 0x22, 'OpenAI key'], ['agents', 0x23, 'Codex'], ['environments', 0x24, 'Workspace'], ['hosts', 0x25, 'host-1'], ['machines', 0x21, 'ada-laptop'], ['templates', 0x26, 'Default']] as const) {
    await page.goto(`/resources/${kind}/${id(tag, 1)}`);
    const main = mainArea(page);
    await main.getByRole('button', { name: `Actions for ${name}`, exact: true }).click();
    const menu = page.getByRole('menu', { name: `Actions for ${name}`, exact: true });
    await expect(menu).toBeVisible();
    const rename = menu.getByRole('menuitem', { name: /^Rename/ });
    if (await rename.count() !== 0) {
      await expect(rename.locator('[data-action-icon="rename"].lucide-pencil')).toBeVisible();
    } else {
      // ResourceHeadingActs folds Rename into a visible Edit form that includes the name.
      await page.keyboard.press('Escape');
      const edit = main.getByRole('button', { name: 'Edit', exact: true });
      await expect(edit.locator('[data-action-icon="edit"].lucide-pencil')).toBeVisible();
      await edit.click();
      const dialog = page.getByRole('dialog');
      const inlineName = main.getByRole('textbox', { name: 'Name', exact: true });
      await expect.poll(async () => await dialog.isVisible() || await inlineName.isVisible()).toBe(true);
      // Hosts edit their name in Settings; other kinds retain their ordinary edit dialog.
      const field = await dialog.isVisible() ? dialog.getByRole('textbox', { name: 'Name', exact: true }) : inlineName;
      await expect(field).toHaveValue(name);
      await expect(field).toBeEnabled();
    }
    await page.keyboard.press('Escape');
  }
});

test('Models remediation opens Models and keeps the creation sign for Add model', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/providers/${id(0x22, 4)}?tab=overview`);
  await expect(page.getByRole('button', { name: /^Models/ }).first().locator('[data-action-icon="models"]')).toBeVisible();
  await page.getByRole('button', { name: /^Models/ }).first().click();
  await expect(page.getByRole('button', { name: 'Add model', exact: true }).locator('[data-action-icon="addModel"]')).toBeVisible();
});

test('thread status is a compact fact whose reason remains under the pointer', async ({ page }) => {
  await signIn(page);
  // This blocked fixture has explanatory status content; a bare waiting state has none.
  await page.goto(`/thread/${ACME.idleThread}`);
  await expect(page.getByRole('button', { name: /^Blocked/ })).toHaveCount(0);
  const state = page.locator('[data-thread-header] [data-state="blocked"]');
  await expect(state).toContainText('Blocked');
  await state.hover();
  // Compact status uses the server's safe display reason; provider particulars remain in its own view.
  await expect(page.getByRole('tooltip', { name: 'Blocked · provider limit reached', exact: true })).toBeVisible();
});

test('device actions and document filters say their effects directly', async ({ page }) => {
  await signIn(page);
  await page.goto('/settings/interface');
  for (const name of ['Restore tips', 'Clear last org', 'Clear device data']) {
    await expect(page.getByRole('button', { name, exact: true }).first()).toBeVisible();
  }
  await page.goto(`/topic/${ACME.fixLogin}?tab=documents`);
  await expect(page.getByRole('checkbox', { name: 'Include child places', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'More ways to make a document', exact: true }).click();
  const privateChoice = page.getByRole('menuitemcheckbox', { name: 'Private', exact: true });
  await expect(privateChoice).toBeVisible();
  await expect(privateChoice).toHaveAccessibleDescription('Create private document');
});

test('a refused document has a truthful leaf and no private name or contextual content', async ({ page }) => {
  await signIn(page);
  await page.goto(`/doc/${id(0x37, 999)}`);
  await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: 'Document unavailable', exact: true })).toBeVisible();
  await expect(path(page)).toHaveCount(0);
  // Independent retained containers remain mounted; no unavailable document body may appear in them.
  await expect(page.locator('[data-document]')).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: 'Document text', exact: true })).toHaveCount(0);
});

test('a known environment gate explains and blocks New shell before an impossible attempt', async ({ page }) => {
  await workspace('/environment', { environment: MAIN_ENVIRONMENT, patch: { terminal: false } });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Terminal');
  await expect(page.getByRole('button', { name: 'Open a new shell', exact: true }).first()).toBeDisabled();
  await expect(page.getByText('Shells are turned off for this environment. Whoever manages it can turn them on in the Environment view.').first()).toBeVisible();
});

for (const [kind, first, next, name] of [
  ['org', '/org/acme', '/org/acme.research', 'Members'],
  ['project', `/project/${ACME.web}`, `/project/${ACME.shop}`, 'Board'],
  ['topic', `/topic/${ACME.fixLogin}`, `/topic/${ACME.migrateCi}`, 'Documents'],
  ['provider', `/resources/providers/${id(0x22, 1)}`, `/resources/providers/${id(0x22, 2)}`, 'Models'],
  ['agent', `/resources/agents/${id(0x23, 1)}`, `/resources/agents/${id(0x23, 4)}`, 'Settings'],
  ['environment', `/resources/environments/${id(0x24, 1)}`, `/resources/environments/${id(0x24, 3)}`, 'Settings'],
  ['host', `/resources/hosts/${id(0x25, 1)}`, `/resources/hosts/${id(0x25, 2)}`, 'What runs on it'],
  ['machine', `/resources/machines/${id(0x21, 1)}`, `/resources/machines/${id(0x21, 2)}`, 'Software'],
  ['template', `/resources/templates/${id(0x26, 1)}`, `/resources/templates/${id(0x26, 2)}`, 'Settings'],
] as const) {
  test(`a ${kind} reading choice stays with its tab across another thing, return and reload`, async ({ page }) => {
    await signIn(page);
    await page.goto(first);
    const choose = () => kind === 'project' ? page.getByRole('radio', { name, exact: true }) : page.getByRole('tab', { name: new RegExp(`^${name}(?: \\d+)?$`) }).first();
    await choose().click();
    const assertChoice = async () => {
      if (kind === 'project') await expect(choose()).toBeChecked();
      else await expect(choose()).toHaveAttribute('aria-selected', 'true');
      if (kind === 'machine') {
        const nav = page.getByRole('navigation', { name: 'Software', exact: true });
        const picker = nav.getByRole('combobox');
        if (await picker.isVisible()) await expect(picker).toHaveValue('Certificate');
        else await expect(nav.getByRole('link', { name: 'Certificate', exact: true })).toHaveAttribute('aria-current', 'page');
      }
    };
    await assertChoice();
    const chosenAddress = page.url();
    const mainTabs = page.getByRole('tablist', { name: 'Main area', exact: true });
    const chosenTab = await mainTabs.getByRole('tab', { selected: true }).getAttribute('id');
    if (chosenTab === null) throw new Error('The selected content tab must keep its identity');
    await page.goto(next);
    // Page sections belong to the addressed thing; another resource starts from its own default.
    if (kind === 'org') await assertChoice();
    else if (kind === 'project') await expect(page.getByRole('radio', { name: 'List', exact: true })).toBeChecked();
    else await expect(page.getByRole('tab', { name: 'Overview', exact: true }).first()).toHaveAttribute('aria-selected', 'true');
    await mainTabs.locator(`[id="${chosenTab}"]`).click();
    await expect(page).toHaveURL(chosenAddress);
    await assertChoice();
    await page.reload();
    await assertChoice();
  });
}

test('a list ancestor returns to the actual filters after reloading a resource', async ({ page }) => {
  await signIn(page);
  await page.goto('/resources/providers?view=usable&q=Team');
  await page.getByRole('link', { name: 'Team Anthropic', exact: true }).click();
  await page.reload();
  await path(page).getByRole('link', { name: 'Providers', exact: true }).click();
  await expect(page).toHaveURL(/view=usable/);
  await expect(page).toHaveURL(/q=Team/);
});

test('the same endpoint disclosure stays expanded on the next provider, return and reload', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/providers/${id(0x22, 1)}?tab=key`);
  await page.locator('summary', { hasText: 'Advanced' }).click();
  // Both API providers are editable by this person; another owner's credentials have no editor to retain.
  for (const route of [`/resources/providers/${id(0x22, 4)}?tab=key`, `/resources/providers/${id(0x22, 1)}?tab=key`]) {
    await page.goto(route);
    await expect(page.locator('details').filter({ has: page.locator('summary', { hasText: 'Advanced' }) })).toHaveAttribute('open', '');
  }
  await page.reload();
  await expect(page.locator('details').filter({ has: page.locator('summary', { hasText: 'Advanced' }) })).toHaveAttribute('open', '');
});

test('only useful available views are offered and unavailable choices remain dormant', async ({ page }) => {
  await signIn(page);
  await expect(bar(page).getByRole('tab', { name: 'Chat', exact: true })).toHaveCount(0);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button', { name: 'Add a view', exact: true }).first().click();
  const chooser = page.getByRole('dialog', { name: 'Views', exact: true });
  await expect(chooser).toBeVisible();
  // A topic without an actual workspace no longer offers a contextless Files view.
  await expect(chooser.locator('[role="option"][data-value="commands:view.open.files"]')).toHaveCount(0);
  await chooser.locator('[role="option"][data-value="commands:view.open.environment"]').click();
  await expect(chooser).toHaveCount(0);
  await expect(bar(page).getByRole('region', { name: 'Environment', exact: true })).toBeVisible();
  await page.goto(`/resources/providers/${id(0x22, 4)}?tab=overview`);
  await expect(page.getByRole('tab', { name: 'Runtime', exact: true })).toHaveCount(0);
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Files');
  await closeThread(ACME.mainThread, 'revoked');
  await expect(page.getByText('You can no longer see this thread')).toBeVisible();
  // The container remains as an unavailable view; its protected file content and acts must disappear.
  await expect(page.getByTestId('file-tree')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New file…', exact: true })).toHaveCount(0);
});

test('phone Comments and files open visible content; navigation reveals the destination and Files can be reopened', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page);
  await page.goto(doc);
  await expect(page.getByRole('textbox', { name: 'Document text', exact: true })).toBeVisible();
  await showWorkbenchView(page, 'Comments');
  await expect(page.getByRole('dialog', { name: 'Document', exact: true }).getByRole('region', { name: 'Comments', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
  await showWorkbenchView(page, 'Files');
  await expect(page.getByRole('dialog', { name: 'Code', exact: true })).toBeVisible();
  await page.locator('[data-container-choice="left:navigation"]').click();
  const navigation = page.getByRole('dialog', { name: 'Navigation', exact: true });
  await navigation.getByRole('treeitem', { name: /\bRepro\b/ }).press('Enter');
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.repro}`));
  await expect(page.getByRole('dialog', { name: 'Code', exact: true })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Repro', exact: true })).toBeVisible();
  await showWorkbenchView(page, 'Files');
  await expect(page.getByRole('dialog', { name: 'Code', exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('dialog', { name: 'Code', exact: true })).toBeVisible();
  const tree = page.getByRole('dialog', { name: 'Code', exact: true }).getByRole('tree');
  const readme = tree.getByRole('treeitem', { name: /^README\.md\b/ });
  const directory = tree.getByRole('treeitem', { name: /^web\b/ });
  await expect.poll(async () => await readme.isVisible() || await directory.isVisible()).toBe(true);
  if (!await readme.isVisible()) await directory.press('ArrowRight');
  await readme.dblclick();
  // Markdown uses the common preview while its file opens as main content.
  const preview = page.getByRole('dialog', { name: 'README.md', exact: true });
  await expect(preview.getByTestId('markdown-text')).toBeVisible();
  await preview.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'README.md', exact: true })).toHaveAttribute('aria-selected', 'true');
  await showWorkbenchView(page, 'Files');
  await expect(page.getByRole('dialog', { name: 'Code', exact: true })).toBeVisible();
});

test('document access and time are facts; privacy actions and their icons say the actual direction', async ({ page }) => {
  await signIn(page);
  await page.goto(doc);
  await expect(page.locator('[data-access-state]')).toHaveText('Uses Fix login permissions');
  await expect(page.getByRole('button', { name: 'Uses Fix login permissions' })).toHaveCount(0);
  await mainArea(page).getByRole('button', { name: 'Actions for Rollout plan', exact: true }).click();
  // Comments is a registered view, opened through Add a view in the phone case above rather than this menu.
  for (const [name, action] of [['History', 'history'], ['Files', 'files'], ['Make private', 'makePrivate'], ['Move to…', 'move'], ['Print', 'print'], ['Copy link', 'copyLink'], ['Delete', 'delete']] as const) {
    await expect(page.getByRole('menuitem', { name, exact: true }).locator(`[data-action-icon="${action}"]`)).toBeVisible();
  }
  await page.getByRole('menuitem', { name: 'Make private', exact: true }).click();
  await page.getByRole('button', { name: 'Make private', exact: true }).click();
  await expect(page.locator('[data-access-state]')).toHaveText('Private');
  await mainArea(page).getByRole('button', { name: 'Actions for Rollout plan', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Use place permissions', exact: true }).locator('[data-action-icon="inheritAccess"]')).toBeVisible();
  await page.getByRole('menuitem', { name: 'Use place permissions', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Use place permissions for this document?', exact: true })).toBeVisible();
  await expect(page.getByText('Ownership stays the same.', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Use place permissions', exact: true }).click();
  await expect(page.locator('[data-access-state]')).toHaveText('Uses Fix login permissions');
});

test('access controls use verbs, retain readable reach and present a folder as a folder', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.payments}`);
  await page.getByRole('button', { name: 'Actions for Payments', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Share…', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'This folder', exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Who has access' })).toBeVisible();
  await page.getByRole('button', { name: 'Options', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'What is shared', exact: true })).toContainText('Contents');
  await expect(page.getByRole('button', { name: 'Make private', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Private', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Permissions', exact: true })).toBeVisible();
});

test('working screens and guides open normally, and a missing article returns to the list', async ({ page }) => {
  await signIn(page);
  await page.getByRole('link', { name: 'Shared with me', exact: true }).click();
  // The screen carries the choice of its Kind control in the address, with the marker that says it is a list's.
  await expect(page).toHaveURL(screenAddress('/shared'));
  await page.goto('/help');
  await page.getByRole('button', { name: 'Getting started', exact: true }).click();
  await expect(page).toHaveURL(/\/help\/getting-started$/);
  const article = page.getByRole('region', { name: 'Getting started', exact: true });
  await expect(article.getByRole('heading', { name: 'Getting started', exact: true })).toBeVisible();
  await page.goto('/help/no-such-guide');
  await expect(page.getByText('There is no article at this address', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'All articles', exact: true }).click();
  await expect(page).toHaveURL(/\/help(?:\?part=articles)?$/);
  await expect(page.getByRole('button', { name: 'Getting started', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'no-such-guide', exact: true })).toHaveCount(0);
  await asAdmin(page, true);
  await page.goto('/admin/storage');
  await expect(page.getByText('Free on the disk', { exact: true }).first()).toBeVisible();
});

/** Language changes use the same Interface control on shipped and development applications, then come back to the screen. */
async function language(page: Page, locale: 'en' | 'zh-CN'): Promise<void> {
  const back = page.url();
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(?:Language|语言)$/ }).click();
  await page.getByRole('option', { name: locale === 'en' ? 'English' : '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
  await page.goto(back);
}

test('two main groups name their Maximize and Restore acts and use the corresponding signs', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
  await showWorkbenchView(page, 'Outline');
  // A single-view title is plain; its named Actions control owns the same keyboard menu.
  await bar(page).getByRole('button', { name: 'Conversation actions', exact: true }).press('Enter');
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).press('ArrowRight');
  await page.getByRole('menuitem', { name: 'Split right', exact: true }).click();
  const outline = page.getByRole('region', { name: /^Main area/ }).getByRole('tab', { name: 'Outline · Main thread · Fix login', exact: true });
  await outline.focus(); await page.keyboard.press('Shift+F10');
  await expect(page.getByRole('menuitem', { name: 'Maximize', exact: true }).locator('[data-action-icon="maximize"]')).toBeVisible();
  await page.getByRole('menuitem', { name: 'Maximize', exact: true }).click();
  await outline.focus(); await page.keyboard.press('Shift+F10');
  await expect(page.getByRole('menuitem', { name: 'Restore', exact: true }).locator('[data-action-icon="restoreGroup"]')).toBeVisible();
  await page.getByRole('menuitem', { name: 'Restore', exact: true }).click();
  await expect(page.getByRole('region', { name: /^Main area/ })).toHaveCount(2);
});

test('approval decisions identify each target and keep their known signs in both languages', async ({ page }) => {
  const first = await askCapability({ title: 'GPU for Grace', requester: 'grace', risk: 'high' });
  const second = await askCapability({ title: 'GPU for Dora', requester: 'dora' });
  const access = await askAccess({ title: 'Vera reads login', requester: 'vera', permissions: ['thread.view'], object: { kind: 'thread', id: ACME.mainThread, name: 'Main thread' } });
  await signIn(page); await page.goto('/approvals');
  for (const locale of ['en', 'zh-CN'] as const) {
    await language(page, locale);
    const approve = locale === 'en' ? 'Approve' : '批准';
    const deny = locale === 'en' ? 'Deny' : '拒绝';
    const join = locale === 'en' ? ' ' : '：';
    // Inbox prose does not rename a capability. Both native GPU targets keep their own request identity.
    for (const capability of [first, second]) {
      const card = page.locator(`[data-approval="${capability.id}"]`);
      await expect(card.getByRole('button', { name: `${approve}${join}GPU`, exact: true }).locator('[data-action-icon="approve"]')).toBeVisible();
      await expect(card.getByRole('button', { name: `${deny}${join}GPU`, exact: true }).locator('[data-action-icon="deny"]')).toBeVisible();
    }
    const card = page.locator(`[data-access-row="${access.id}"]`);
    await card.getByRole('button', { name: locale === 'en' ? 'Other…' : '其他…', exact: true }).click();
    await expect(card.getByRole('button', { name: `${approve}${join}Vera reads login`, exact: true }).locator('[data-action-icon="approve"]')).toBeVisible();
    await expect(card.getByRole('button', { name: `${deny}${join}Vera reads login`, exact: true }).locator('[data-action-icon="deny"]')).toBeVisible();
    await expect(page.getByRole('button', { name: approve, exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: deny, exact: true })).toHaveCount(0);
  }
});

test('a muted human conversation offers the inverse Unmute act with its BellRing sign', async ({ page }) => {
  await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Review crew' });
  await signIn(page); await page.goto('/org/acme/messages');
  const row = page.getByRole('list', { name: 'Chats', exact: true }).locator('li').first();
  await row.hover(); await row.getByRole('button', { name: /^Actions for / }).click();
  await page.getByRole('menuitem', { name: 'Mute', exact: true }).click();
  await row.hover(); await row.getByRole('button', { name: /^Actions for / }).click();
  const unmute = page.getByRole('menuitem', { name: 'Unmute', exact: true });
  await expect(unmute.locator('[data-action-icon="unmute"].lucide-bell-ring')).toBeVisible();
  await expect(unmute.locator('.lucide-bell-off')).toHaveCount(0);
});

test('the selected permitted conversation has a named leaf even when it is outside the caller own chat list', async ({ page }) => {
  const mine = await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Review crew' });
  const other = await chats('/chat', { org: ACME.org, members: ['dora', 'linus'], name: 'Release review' });
  // This reader explicitly enters inspection authority for the permitted nonmember conversation.
  await signInForManagement(page);
  for (const [chat, name] of [[mine.chat, 'Review crew'], [other.chat, 'Release review']] as const) {
    await page.goto(`/org/acme/messages?chat=${String(chat)}`);
    await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: `Chat · ${name}`, exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(path(page).getByRole('link', { name, exact: true })).toHaveCount(0);
    await expect(path(page).getByRole('link', { name: 'Messages', exact: true })).toHaveAttribute('href', '/org/acme/messages');
  }
});

test('creation entrances carry the common creation sign with their explicit object names', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme');
  await expect(page.getByRole('button', { name: 'New project', exact: true }).locator('[data-action-icon="create"]')).toBeVisible();
  await page.getByRole('button', { name: 'Actions for Acme', exact: true }).last().click();
  await expect(page.getByRole('menuitem', { name: 'New project…', exact: true })).toHaveCount(0);
  await expect(page.getByRole('menuitem', { name: 'New department…', exact: true }).locator('[data-action-icon="create"]')).toBeVisible();
  await page.keyboard.press('Escape'); await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('button', { name: 'New topic', exact: true }).locator('[data-action-icon="create"]')).toBeVisible();
  await page.getByRole('button', { name: 'Actions for Web', exact: true }).last().click();
  await expect(page.getByRole('menuitem', { name: 'New folder…', exact: true }).locator('[data-action-icon="createFolder"]')).toBeVisible();
});

test('the empty document creation entrance names its chosen private creation mode and creates a private document', async ({ page }) => {
  await signIn(page); await page.goto(`/topic/${ACME.upgradeDeps}?tab=documents`);
  await page.getByRole('button', { name: 'More ways to make a document', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Private', exact: true }).click();
  await page.keyboard.press('Escape');
  const create = page.getByRole('button', { name: 'New private document', exact: true });
  // The empty list uses its single shared toolbar creation entrance.
  await expect(create).toHaveCount(1);
  await create.click();
  await expect(page).toHaveURL(/\/doc\//);
  await expect(page.locator('[data-access-state]')).toHaveText('Private');
});


for (const [target, route] of [
  ['topic', `/topic/${ACME.fixLogin}?tab=documents`],
  ['resource kinds', '/resources/environments'],
] as const) test(`the selected ${target} phone tab stays fully readable after resize and a language change`, async ({ page }) => {
  await signIn(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(route);
  await page.setViewportSize({ width: 390, height: 844 });
  for (const locale of ['en', 'zh-CN', 'en'] as const) {
      await language(page, locale);
      const isTopic = route.includes('/topic/');
      const selected = isTopic ? page.getByRole('tab', { name: locale === 'en' ? 'Documents' : '文档', exact: true }).first()
        : page.getByRole('navigation', { name: locale === 'en' ? 'Kinds of resources' : '资源的种类', exact: true }).getByRole('link', { name: locale === 'en' ? 'Environments' : '环境', exact: true });
      await expect(selected).toHaveAttribute(isTopic ? 'aria-selected' : 'aria-current', isTopic ? 'true' : 'page');
      await expect.poll(() => selected.evaluate((tab) => {
        const row = tab.parentElement!;
        const room = row.getBoundingClientRect();
        const item = tab.getBoundingClientRect();
        // Page tabs have no edge fade; their own reveal() keeps the selected text inside the physical strip.
        return item.left >= room.left - 1 && item.right <= room.right + 1;
      })).toBe(true);
  }
});

for (const [route, english, chinese] of [
  ['/settings/tokens', 'Your settings', '你的设置'],
  ['/org/acme/settings/roles', 'Sections of the settings of Acme', 'Acme设置的各个部分'],
  [`/project/${ACME.web}/settings?section=destinations`, 'Sections of the settings of Web', 'Web设置的各个部分'],
] as const) test(`the selected section of ${english} stays fully readable after a phone language change`, async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page); await page.goto(route);
  for (const locale of ['en', 'zh-CN', 'en'] as const) {
      await language(page, locale);
      const nav = page.getByRole('navigation', { name: locale === 'en' ? english : chinese, exact: true });
      const chosen = nav.locator('a[aria-current="page"]');
      await expect(chosen).toHaveCount(1);
      const picker = nav.getByRole('combobox');
      const visible = await picker.isVisible() ? picker : chosen;
      if (await picker.isVisible()) await expect(picker).toHaveValue((await chosen.textContent())?.trim() ?? '');
      await expect.poll(() => visible.evaluate((link) => {
        const row = link.closest('nav')!;
        const room = row.getBoundingClientRect();
        const item = link.getBoundingClientRect();
        return item.left >= room.left - 1 && item.right <= room.right + 1;
      })).toBe(true);
  }
});

for (const locale of ['en', 'zh-CN'] as const) test(`known secondary navigation and creation commands show their signs in ${locale}`, async ({ page }) => {
  await signIn(page);
    const english = locale === 'en';
    await page.goto(`/thread/${ACME.mainThread}`); await language(page, locale);
    await expect(page.locator('[data-thread-header]').getByRole('button', { name: english ? 'Find in the conversation' : '在对话中查找', exact: true }).locator('[data-action-icon="search"]')).toBeVisible();
    await page.goto('/resources/providers'); await language(page, locale);
    await page.getByRole('button', { name: english ? 'Add provider' : '添加提供方', exact: true }).click();
    await expect(page.getByRole('button', { name: english ? 'Next' : '下一步', exact: true }).locator('[data-action-icon="next"]')).toBeVisible();
    await page.goto('/resources/agents'); await language(page, locale);
    await page.getByRole('button', { name: english ? 'Add agent profile' : '添加智能体配置', exact: true }).click();
    await expect(page.getByRole('button', { name: english ? 'Add agent profile' : '添加智能体配置', exact: true }).locator('[data-action-icon="create"]')).toBeVisible();
    await page.goto('/resources/hosts'); await language(page, locale);
    await page.getByRole('button', { name: english ? 'Add host' : '添加主机', exact: true }).click();
    await expect(page.getByRole('dialog').getByRole('button', { name: english ? 'Show command' : '显示命令', exact: true }).locator('[data-action-icon="show"]')).toBeVisible();
    await page.goto('/org/acme/settings/roles'); await language(page, locale);
    await page.getByRole('button', { name: english ? 'View defaults' : '查看默认权限', exact: true }).first().click();
    await expect(page.getByRole('tab', { name: english ? 'Defaults' : '默认设置', exact: true })).toHaveAttribute('aria-selected', 'true');
    await page.keyboard.press('Escape');
    await page.goto(`/topic/${ACME.fixLogin}`); await language(page, locale);
    await page.getByRole('button', { name: english ? 'Actions for Fix login' : 'Fix login 的操作', exact: true }).last().click();
    await page.getByRole('menuitem', { name: english ? 'Share…' : '共享…', exact: true }).click();
    await page.getByRole('button', { name: english ? 'Permissions' : '权限明细', exact: true }).click();
    await expect(page.getByRole('button', { name: english ? 'Back to the short form' : '返回简版', exact: true }).locator('[data-action-icon="backPermissions"]')).toBeVisible();
});


test('the Chinese guide path uses its catalogue name and opens the authored body', async ({ page }) => {
  await signIn(page); await page.goto('/help/resources'); await language(page, 'zh-CN');
  await expect(page.getByRole('tablist', { name: '主区域', exact: true }).getByRole('tab', { name: '资源与移动', exact: true })).toBeVisible();
  await expect(path(page).getByRole('link', { name: '资源与移动', exact: true })).toHaveCount(0);
  const article = mainArea(page).getByRole('region', { name: '资源与移动', exact: true }).last();
  // This guide now has an authored Chinese body, so it no longer uses the English fallback notice.
  await expect(article.getByRole('heading', { name: '资源与移动', exact: true, level: 1 })).toBeVisible();
  await expect(article.getByRole('heading', { name: '挂载和工具服务器', exact: true })).toBeVisible();
  await expect(page.getByText('本文以英文显示。', { exact: true })).toHaveCount(0);
});


test('an edited document title remains fully visible when the desktop inspector is resized', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await signIn(page); await page.goto(doc);
  const title = page.getByRole('textbox', { name: 'Title', exact: true });
  const text = 'Rollout plan for the login changes, canary verification, shared ownership, rollback steps and the final review of every session before the feature flag is opened to the whole team';
  await title.fill(text);
  await expect.poll(() => title.evaluate((field) => field.scrollHeight - field.clientHeight)).toBeLessThanOrEqual(1);
  const before = await title.boundingBox();
  const inspector = await page.getByRole('region', { name: 'Right bar', exact: true }).boundingBox();
  for (const separator of await page.getByRole('separator').all()) {
    const box = await separator.boundingBox();
    if (box !== null && inspector !== null && box.height > 200 && Math.abs(box.x - inspector.x) < 12) {
      await page.mouse.move(box.x + box.width / 2, box.y + 60); await page.mouse.down();
      await page.mouse.move(box.x - 180, box.y + 60, { steps: 5 }); await page.mouse.up(); break;
    }
  }
  const after = await title.boundingBox();
  expect(after?.width).toBeLessThan((before?.width ?? 0) - 80);
  await expect.poll(() => title.evaluate((field) => field.scrollHeight - field.clientHeight)).toBeLessThanOrEqual(1);
  await expect(title).toHaveValue(text);
  await expect.poll(async () => (await title.boundingBox())?.height ?? 0).toBeGreaterThan(before?.height ?? 0);
  await language(page, 'zh-CN');
  await expect.poll(() => page.getByRole('textbox', { name: '标题', exact: true }).evaluate((field) => field.scrollHeight - field.clientHeight)).toBeLessThanOrEqual(1);
});

test('the waiting notice names showing the approval and gives that act the common Show sign', async ({ page }) => {
  await playScene(ACME.mainThread, 'everything'); await signIn(page); await page.goto(`/thread/${ACME.mainThread}`);
  for (const locale of ['en', 'zh-CN'] as const) {
    await language(page, locale);
    const shows = page.getByRole('button', { name: locale === 'en' ? 'Show waiting approval' : '查看待处理的审批', exact: true });
    // The complete scene now has several independent waiting notices; audit every offered action.
    await expect(shows.first()).toBeVisible();
    for (const show of await shows.all()) await expect(show.locator('[data-action-icon="show"]')).toBeVisible();
  }
});


test('the agent-sharing entrance uses the same named Share2 act in both languages', async ({ page }) => {
  await signIn(page);
  for (const locale of ['en', 'zh-CN'] as const) {
    const english = locale === 'en';
    await page.goto(`/topic/${ACME.fixLogin}`); await language(page, locale);
    await page.getByRole('button', { name: english ? 'Actions for Fix login' : 'Fix login 的操作', exact: true }).last().click();
    await page.getByRole('menuitem', { name: english ? 'Share…' : '共享…', exact: true }).click();
    await page.getByRole('tab', { name: english ? 'Agents' : '智能体', exact: true }).click();
    const share = page.getByRole('button', { name: english ? 'Share with agents…' : '与智能体共享…', exact: true });
    await expect(share.locator('[data-action-icon="shareAgents"]')).toBeVisible();
    await share.click();
    await expect(page.getByRole('dialog', { name: english ? 'Share “Fix login” with agents' : '把“Fix login”共享给智能体', exact: true })).toBeVisible();
    await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
  }
});

for (const locale of ['en', 'zh-CN'] as const) test(`All articles opens Articles and clears the retained contextual search in ${locale}`, async ({ page }) => {
  // Contextual hints use the main cell's width: leave room for Help beside this thread.
  await page.setViewportSize({ width: 1920, height: 1080 });
  await signIn(page);
  const english = locale === 'en';
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
  await language(page, locale);
  await showWorkbenchView(page, 'Help');
  // This screen and its retained query belong to contextual Help; page Help owns a separate article search.
  const help = page.getByRole('region', { name: english ? 'Help' : '帮助', exact: true }).last();
  await help.getByRole('tab', { name: english ? 'This screen' : '当前屏幕', exact: true }).click();
  const field = help.getByRole('searchbox');
  const guide = english ? 'Finding things' : '查找内容';
  await field.fill('retained search');
  // A contextual guide keeps the prior search; typing a new search after opening it deliberately closes the article.
  await page.locator('[data-main-area] [data-help-control="upload-message-files"]').getByRole('button').click();
  await page.getByRole('button', { name: english ? 'Read more' : '了解更多', exact: true }).click();
  const back = help.getByRole('button', { name: english ? 'All articles' : '全部文章', exact: true });
  await expect(back).toBeVisible();
  await expect(field).toHaveValue('retained search');
  await expect(back.locator('[data-action-icon="back"]')).toBeVisible();
  await back.click();
  await expect(page).toHaveURL((url) => url.pathname === `/thread/${ACME.mainThread}`);
  await expect(help.getByRole('tab', { name: english ? /^Articles/ : /^文章/ })).toHaveAttribute('aria-selected', 'true');
  await expect(field).toHaveValue('');
  await expect(help.getByRole('button', { name: new RegExp(guide) })).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) test(`opening another shell after exit keeps the creation sign in ${locale}`, async ({ page }) => {
  await signIn(page);
    const name = locale === 'en' ? 'Open a new shell' : '打开新的终端';
    await workspace('/reset'); await emit(ACME.mainThread, { thread: { state: 'idle', directory: 'web' } });
    await page.goto(`/thread/${ACME.mainThread}`); await language(page, locale);
    await showWorkbenchView(page, 'Terminal');
    await page.getByRole('button', { name, exact: true }).first().click();
    await expect(page.locator('.xterm-rows')).toContainText('$');
    await page.locator('.xterm-helper-textarea').focus(); await page.keyboard.type('exit 3\n');
    const another = page.locator('[data-view-actions="terminal"]').getByRole('button', { name, exact: true });
    await expect(page.getByRole('button', { name, exact: true })).toHaveCount(1);
    await expect(another.locator('[data-action-icon="newShell"].lucide-plus')).toBeVisible();
    await another.click(); await expect(page.locator('.xterm-rows')).toContainText('$');
});

test('secret write and replacement carry the shared Save sign in both languages', async ({ page }) => {
  await signIn(page);
  for (const locale of ['en', 'zh-CN'] as const) {
    const english = locale === 'en';
    await page.goto(`/project/${ACME.web}/settings?section=defaults`); await language(page, locale);
    await page.getByRole('button', { name: english ? 'New secret…' : '新建密钥…', exact: true }).click();
    const write = page.getByRole('button', { name: english ? 'Write secret' : '写入密钥', exact: true });
    await expect(write.locator('[data-action-icon="save"].lucide-save')).toBeVisible();
    await page.keyboard.press('Escape');
  }
});
