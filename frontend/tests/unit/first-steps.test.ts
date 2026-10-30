/** A readable saved org, one team and personal fallback, preserved choices, and scoped observed facts. */
import { describe, expect, it } from 'vitest';

import { ApiError } from '@/api/client';
import { firstStepsAccessRefused } from '@/api/firstSteps';
import { firstStepsChoice, internalAction, landingOrg, needsFirstSteps, withFirstSteps } from '@/features/onboarding/state';
import type { Org } from '@/generated/api-types';

import { ACME, me, org, personalHomeOf, userRef } from '../doubles/fixtures';

const world = () => {
  const person = { me: me(1, 'ada', 'Ada Lovelace') };
  const owner = userRef(person.me);
  const access = { admin_only: [], participating: true, administers: true, owns: true, role: 'admin' as const, role_from: null, permissions: ['org.view'], subscription: null, effective_subscription: 'off' as const };
  const orgs = [org(personalHomeOf(person.me), 'ada', 'Ada', owner, { org_kind: 'personal', access }), org(ACME.org, 'acme', 'Acme', owner, { access })];
  return { held: { orgsOf: () => orgs }, person };
};

describe('landing without a guessed team or private name', () => {
  it('never treats an own space as missing team setup, even when its provider is unconfigured', () => {
    const missing = { org: null, work_mode: null, invitation_roles: [], start_here: null, as_of: '2026-10-15T22:20:00Z',
      steps: [{ id: 'provider' as const, fact: 'not_done' as const, state: 'open' as const, scripted: false, source: null, action: null }] };
    expect(needsFirstSteps(missing, { org_kind: 'personal', member_count: 1 })).toBe(false);
    expect(needsFirstSteps(missing, { org_kind: 'team', member_count: 1 })).toBe(true);
  });
  it('reports a missing setup route as a failed load, while an actual forbidden read remains a refusal', () => {
    const missing = new ApiError({ type: 'orch:error:not_found', title: 'Not found', code: 'not_found', status: 404, detail: 'This route is missing.', request_id: 'missing-route' }, null);
    const refused = new ApiError({ ...missing.problem, type: 'orch:error:authz.denied', title: 'Denied', code: 'authz.denied', status: 403 }, null);
    expect(firstStepsAccessRefused(missing)).toBe(false);
    expect(firstStepsAccessRefused(new Error('offline'))).toBe(false);
    expect(firstStepsAccessRefused(refused)).toBe(true);
  });

  it('opens the saved readable org before a unique team and personal space', () => {
    const { held, person } = world();
    const orgs = held.orgsOf();
    expect(landingOrg(orgs, person.me.personal_org_id, person.me.personal_org_id)?.id).toBe(person.me.personal_org_id);
    expect(landingOrg(orgs, person.me.personal_org_id, null)?.id).toBe(ACME.org);
  });

  it('ignores an administrable private saved org and picks personal when multiple teams are readable', () => {
    const { held, person } = world();
    const orgs = held.orgsOf();
    const team = orgs.find((org) => org.id === ACME.org);
    if (team === undefined) throw new Error('the fixture needs a team');
    const privateTeam: Org = { ...team, id: 'private', name: 'Do not disclose', access: { ...team.access, permissions: ['members.manage'] } };
    const second: Org = { ...team, id: 'second', address: 'second', name: 'Second team' };
    expect(landingOrg([...orgs, privateTeam, second], person.me.personal_org_id, 'private')?.id).toBe(person.me.personal_org_id);
  });

  it('preserves unrelated help when changing or dismissing first steps', () => {
    const state = { help: { finished: ['first-thread'], closed: ['hint.queue'] }, sidebar: { folded: ['recent'] } };
    const choice = { org_id: ACME.org, work_mode: 'team' as const, dismissed: true };
    expect(withFirstSteps(state, choice)).toEqual({ ...state.help, first_steps: choice });
    expect(firstStepsChoice({ help: withFirstSteps(state, choice) })).toEqual(choice);
    expect(firstStepsChoice({ help: { first_steps: { work_mode: 'guessed', dismissed: 'yes' } } })).toEqual({ org_id: null, work_mode: null, dismissed: false });
  });

  it('admits local actions and refuses external and disguised addresses', () => {
    expect(internalAction('/settings/notifications')).toBe('/settings/notifications');
    for (const address of ['//elsewhere.example/path', '/\\elsewhere.example', 'https://elsewhere.example', 'javascript:alert(1)']) expect(internalAction(address)).toBeNull();
  });
});

