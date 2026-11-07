/** Typed opening data for screen-layout tests whose native section readers are controlled separately. */
import { resourceRows } from '@/api/listed';
import type { FirstSteps, NameSearchPage, Node, Org, OrgOverview } from '@/generated/api-types';

export function overviewFixture(org: Org, children: readonly Node[] = [], recent: NameSearchPage = { items: [], partial: false }, setup: FirstSteps | null = null): OrgOverview {
  return { org, children: { items: resourceRows([...children]), next_cursor: null }, recent,
    first_steps: setup === null ? { status: 'omitted' } : { status: 'ready', data: setup },
    outside_status: { status: 'omitted' }, usage: { status: 'omitted' }, activity: { status: 'omitted' }, outside_openings: { status: 'omitted' } };
}
