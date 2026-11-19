/** Opens a real workbench view through the same contextual chooser a person uses. */
import { expect, type Page } from '@playwright/test';

const VIEWS: Readonly<Record<string, string>> = {
  Outline: 'outline', History: 'history', Chat: 'chat', Documents: 'documents', People: 'people',
  Changes: 'changes', Files: 'files', Git: 'git', Environment: 'environment', Processes: 'processes', Resources: 'resources',
  Ports: 'ports', Browsers: 'browsers', Displays: 'displays', Terminal: 'terminal', Parameters: 'parameters', Usage: 'usage',
  Knowledge: 'knowledge', 'Given to the agent': 'given', Given: 'given', Diagnostics: 'diagnostics', Browser: 'browser', VNC: 'vnc', Help: 'help', Comments: 'comments', Links: 'links',
};

export async function showWorkbenchView(page: Page, name: string | RegExp): Promise<void> {
  const pattern = typeof name === 'string' ? null : new RegExp(name.source, name.flags.replace('g', ''));
  const id = typeof name === 'string' ? VIEWS[name] : Object.entries(VIEWS).find(([title]) => pattern?.test(title))?.[1];
  if (id === undefined) throw new Error(`No registered test view matches ${String(name)}`);
  const add = page.getByRole('button', { name: /^(Add a view|添加视图)$/ }).first();
  const narrow = page.locator('[data-narrow-containers] button').first();
  // Admission/route loading is not a phone layout. Wait for an actual frame control before choosing its workflow.
  await expect.poll(async () => await add.isVisible() || await narrow.isVisible()).toBe(true);
  if (!await add.isVisible()) await narrow.click();
  await page.getByRole('button', { name: /^(Add a view|添加视图)$/ }).first().click();
  const chooser = page.getByRole('dialog', { name: /^(Views|视图)$/ });
  await expect(chooser).toBeVisible();
  await chooser.locator(`[role="option"][data-value="commands:view.open.${id}"]`).click();
  await expect(chooser).toHaveCount(0);
}
