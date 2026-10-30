/** Portable first-steps choices and safe org landing without a guessed team. */
import type { FirstSteps, Org } from '@/generated/api-types';

/** Unknown facts prove no missing setup; a known peer, a personal root or solo needs no invitation. */
export function needsFirstSteps(data: FirstSteps, org: Pick<Org, 'org_kind' | 'member_count'>): boolean {
  if (org.org_kind === 'personal') return false;
  // The member count can include the owner; more than one already proves a teammate is present.
  const needsMembers = org.member_count <= 1 && data.work_mode !== 'solo';
  return data.steps.some((step) => step.state === 'open' && (step.id === 'provider' || (step.id === 'teammate' && needsMembers)));
}

export interface FirstStepsChoice {
  org_id: string | null;
  work_mode: 'team' | 'solo' | null;
  dismissed: boolean;
}

export const record = (value: unknown): Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};

export function firstStepsChoice(state: Record<string, unknown> | undefined): FirstStepsChoice {
  const choice = record(record(state?.help).first_steps);
  return {
    org_id: typeof choice.org_id === 'string' ? choice.org_id : null,
    work_mode: choice.work_mode === 'team' || choice.work_mode === 'solo' ? choice.work_mode : null,
    dismissed: choice.dismissed === true,
  };
}

/** Preserve tutorials, closed hints, folding and visibility when writing our own choice. */
export function withFirstSteps(state: Record<string, unknown>, choice: FirstStepsChoice): Record<string, unknown> {
  return { ...record(state.help), first_steps: choice };
}

export function savedOrg(state: Record<string, unknown> | undefined): string | null {
  const id = record(state?.sidebar).last_org_id;
  return typeof id === 'string' ? id : null;
}

/** Org lists may contain administrable private rows; those are not readable landing targets. */
export function landingOrg(orgs: readonly Org[], personalId: string | null | undefined, last: string | null): Org | undefined {
  const readable = orgs.filter((org) => org.access.permissions.includes('org.view'));
  const saved = readable.find((org) => org.id === last);
  if (saved !== undefined) return saved;
  const teams = readable.filter((org) => org.org_kind === 'team' && org.depth === 0);
  if (teams.length === 1) return teams[0];
  return readable.find((org) => org.id === personalId);
}

/** No external or protocol-relative address can become a first-step action. */
export function internalAction(url: string): string | null {
  if (!url.startsWith('/') || url.startsWith('//') || url.includes('\\')) return null;
  try {
    const parsed = new URL(url, 'https://orch.invalid');
    return parsed.origin === 'https://orch.invalid' ? `${parsed.pathname}${parsed.search}${parsed.hash}` : null;
  } catch {
    return null;
  }
}

