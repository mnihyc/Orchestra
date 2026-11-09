/** The existing plan lists name retained resources, while additions and losses keep their own places. */
import { cleanup, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { PlanLists } from '@/features/change/Lists';
import { addedResourcesOf, changesOf, endsOf, keptResourcesOf, removedResourcesOf, staysOf, type PlanContext } from '@/features/change/plan';
import { i18n, setLocale } from '@/i18n';
import { planMount, resourcePlan } from './plan-resource-fixtures';
import { show } from './support';
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const compact of [false, true]) it('names every reported resource in its one plan section ('+locale+', compact='+String(compact)+')', async () => {
  await setLocale(locale);
  const plan = resourcePlan('Work');
  plan.mounts.push(planMount('attach', 'New mount'), planMount('detach', 'Old mount'));
  plan.mcp.push({ id: 'new', name: 'New tools', imported: false, own: false, fate: 'new', reason: null }, { id: 'lost', name: 'Old tools', imported: true, own: true, fate: 'lost', reason: null });
  const context: PlanContext = { kind: 'switch', thread: { name: 'Work', model: 'm1', modelDisplay: 'M1', effort: null, owner: plan.owner.after }, me: plan.owner.after.id, locale };
  show(<PlanLists compact={compact} changes={compact ? addedResourcesOf(plan, i18n.t) : changesOf(plan, context, i18n.t)} stays={compact ? keptResourcesOf(plan, i18n.t) : staysOf(plan, context, i18n.t)}
    ends={compact ? removedResourcesOf(plan, i18n.t) : endsOf(plan, context, i18n.t)} />);
  const stays = screen.getByRole('region', { name: i18n.t('change.lists.stays') });
  expect(stays).toHaveTextContent(i18n.t('mcpPlan.kept', { name: 'Research' }));
  expect(stays).toHaveTextContent('Shared data'); expect(within(stays).getAllByRole('listitem')).toHaveLength(compact ? 2 : 6);
  const changes = screen.getByRole('region', { name: i18n.t('change.lists.changes') });
  expect(changes).toHaveTextContent('New mount'); expect(changes).toHaveTextContent('New tools'); expect(changes).not.toHaveTextContent('Shared data');
  const ends = screen.getByRole('region', { name: i18n.t('change.lists.ends') });
  expect(ends).toHaveTextContent('Old mount'); expect(ends).toHaveTextContent('Old tools'); expect(ends).not.toHaveTextContent('Research');
  expect(screen.getAllByText('Research', { exact: true })).toHaveLength(1);
});
it('does not invent a kept resource when the plan reports none', () => {
  show(<PlanLists compact changes={[]} stays={keptResourcesOf({ mounts: [], mcp: [] }, i18n.t)} ends={[]} />);
  expect(screen.queryByRole('region', { name: i18n.t('change.lists.stays') })).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps the reported owner as the shared inspectable person in the plan (${locale})`, async () => {
  await setLocale(locale);
  const owner = resourcePlan('Work').owner.after;
  show(<PlanLists changes={[]} ends={[]} stays={[{ id: 'owner', tone: 'plain', people: [owner], text: i18n.t('change.env.plan.staysOwners', { name: owner.display_name }) }]} />);
  const kept = screen.getByRole('region', { name: i18n.t('change.lists.stays') });
  expect(within(kept).getByRole('button', { name: owner.display_name, exact: true })).toBeVisible();
  expect(kept).toHaveTextContent(owner.display_name);
});
