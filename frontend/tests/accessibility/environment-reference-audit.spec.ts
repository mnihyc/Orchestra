/** A case-local reference allowance never weakens the unchanged duplicate-text rule. */
import { expect, test } from '@playwright/test';
import { inspectLayout } from '../../e2e/layout-rules-dom';
import { environmentReferenceAllowance, type EnvironmentReferenceProof } from '../e2e/environment-reference-audit';

const proof: EnvironmentReferenceProof = { origin: 'http://orch.test', environment: { id: 'environment-one', name: 'Workspace' },
  tab: { id: 'tab-environment', url: 'http://orch.test/resources/environments/environment-one?tab=tools', text: 'Workspace' },
  status: { url: '/resources/environments/environment-one', label: 'Environment: Workspace', text: 'Workspace' } };

test('allows exactly one verified navigation/status pair and still rejects third copies and wrong subjects', async ({ page }) => {
  await page.setContent('<style>body{background:white;color:black}</style><div role="tablist"><div role="tab" id="tab-environment"><span data-view-tab-title>Workspace</span></div></div><footer><div data-status-place="bar"><span data-status-item="environment"><button aria-label="Environment: Workspace"><span class="truncate">Workspace</span></button></span></div></footer>');
  const audit = async (current: EnvironmentReferenceProof) => {
    const allowance = environmentReferenceAllowance(current);
    return page.evaluate(inspectLayout, { duplicateAllowList: allowance === undefined ? [] : [allowance], rawValues: [] });
  };
  const unqualified = await page.evaluate(inspectLayout, { duplicateAllowList: [], rawValues: [] });
  expect(unqualified.faults.filter((row) => row.rule === 'duplicate-text').map((row) => row.text)).toEqual(['Workspace']);
  const allowed = await audit(proof);
  expect(allowed.faults.filter((row) => row.rule === 'duplicate-text')).toEqual([]);
  expect(allowed.allowances).toHaveLength(1);
  for (const wrong of [{ ...proof, tab: { ...proof.tab, url: '/resources/environments/another-environment' } },
    { ...proof, status: { ...proof.status, url: '/resources/environments/another-environment' } },
    { ...proof, status: { ...proof.status, url: 'http://another-origin.test/resources/environments/environment-one' } }]) {
    expect(environmentReferenceAllowance(wrong)).toBeUndefined();
    expect((await audit(wrong)).faults.some((row) => row.rule === 'duplicate-text' && row.text === 'Workspace')).toBe(true);
  }
  await page.locator('body').evaluate((body) => { const third = document.createElement('span'); third.textContent = 'Workspace'; body.append(third); });
  expect((await audit(proof)).faults.some((row) => row.rule === 'duplicate-text' && row.text === 'Workspace')).toBe(true);
});
