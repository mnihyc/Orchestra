/** Facts stay distinct from actions, solo omits inviting, roles describe current defaults, and guides are ordinary links. */
import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ApiError } from '@/api/client';
import { FirstStepsFailure, StartHere, StepList } from '@/features/onboarding/Checklist';
import { RoleSummary } from '@/features/onboarding/RoleSummary';
import { show } from './support';
import { ACME } from '../doubles/fixtures';
import type { FirstSteps, InvitationRoleSummary } from '@/generated/api-types';

function projection(): FirstSteps {
  const place = { id: ACME.org, kind: 'org' as const, name: 'Acme', redacted: false };
  return { org: place, work_mode: 'team', invitation_roles: [], start_here: null, as_of: '2026-10-13T14:00:00Z', steps: [
    { id: 'team', fact: 'team_chosen', state: 'done', scripted: false, source: place, action: null },
    { id: 'provider', fact: 'key_saved', state: 'done', scripted: false, source: place, action: { url: '/resources/providers/example', allowed: true, reason: null } },
    { id: 'work', fact: 'not_done', state: 'open', scripted: false, source: null, action: { url: '/org/acme', allowed: true, reason: null } },
    { id: 'teammate', fact: 'invitation_waiting', state: 'open', scripted: false, source: place, action: { url: '/org/acme?tab=members', allowed: true, reason: null } },
  ] };
}

describe('the observed checklist', () => {
  it('shows an unavailable projection as a load failure without protocol identifiers, and keeps actual refusals distinct', () => {
    const missing=new ApiError({type:'orch:error:not_found',title:'Not found',code:'not_found',status:404,detail:'No such route',request_id:'missing-checklist'},null);
    const view=show(<FirstStepsFailure error={missing} />);
    expect(screen.getByRole('alert')).toHaveTextContent('Could not check this org');
    expect(screen.getByRole('alert')).toHaveTextContent("First steps are unavailable. Try again or ask the system's administrator.");
    expect(screen.getByRole('alert')).not.toHaveTextContent('not open to you');
    expect(screen.getByRole('alert')).not.toHaveTextContent('First steps need access');
    expect(screen.getByRole('alert')).not.toHaveTextContent(/missing-checklist|not_found|No such route/);
    view.rerender(<FirstStepsFailure error={new ApiError({...missing.problem,type:'orch:error:authz.denied',code:'authz.denied',status:403},null)} />);
    expect(screen.getByRole('alert')).toHaveTextContent('First steps need access');
    expect(screen.getByRole('alert')).not.toHaveTextContent('Could not check this org');
    expect(screen.getByRole('alert')).not.toHaveTextContent(/missing-checklist|authz.denied/);
  });

  it('shows Key saved as a fact and Open provider as the action, never a model success', () => {
    show(<StepList data={projection()} />);
    expect(screen.getByText('Key saved')).toBeVisible();
    expect(screen.getByRole('link', { name: 'Open provider' })).toHaveAttribute('href', expect.stringContaining('/resources/providers/'));
    expect(screen.getByRole('link', { name: 'Open provider' }).querySelector('[data-action-icon="open"]')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Key saved' })).not.toBeInTheDocument();
    expect(screen.queryByText(/successful model/i)).not.toBeInTheDocument();
  });

  it('offers no later setup acts before the team or solo choice', () => {
    show(<StepList data={{ ...projection(), work_mode: null }} />);
    expect(screen.queryByText('Choose your org')).not.toBeInTheDocument();
    expect(document.querySelector('[data-step="team"]')).not.toBeInTheDocument();
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  });

  it('does not offer a teammate for solo and keeps a scripted result labelled', () => {
    const data = projection();
    const work = data.steps.find((step) => step.id === 'work');
    if (work !== undefined) Object.assign(work, { scripted: true, state: 'done', fact: 'answered_once' });
    show(<StepList data={{ ...data, work_mode: 'solo' }} />);
    expect(document.querySelector('[data-step="teammate"]')).not.toBeInTheDocument();
    expect(screen.getByText('Scripted')).toBeVisible();
  });

  it('says why an act is refused and offers no active link for it', () => {
    const data = projection();
    const provider = data.steps.find((step) => step.id === 'provider');
    if (provider !== undefined) provider.action = { url: '/resources/providers', allowed: false, reason: 'Ask Lee to configure providers here.' };
    show(<StepList data={data} />);
    expect(screen.getByText('Ask Lee to configure providers here.')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Set up provider' })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByRole('link', { name: 'Open provider' })).not.toBeInTheDocument();
  });

  it('a configured outward way never claims that a device received anything', () => {
    const data = projection();
    data.steps.push({ id: 'away', fact: 'outward_enabled', state: 'done', scripted: false, source: null, action: { url: '/settings/notifications', allowed: true, reason: 'Not tested' } });
    show(<StepList data={data} />);
    expect(screen.getByText('Configured')).toBeVisible();
    expect(screen.getByText(/does not confirm receipt on a device/)).toBeVisible();
    expect(screen.getByText('Not tested')).toBeVisible();
    expect(screen.getByRole('link', { name: 'Notification settings' })).toHaveAttribute('href', '/settings/notifications');
    expect(screen.getByRole('link', { name: 'Notification settings' }).querySelector('[data-action-icon="notificationSettings"]')).toBeInTheDocument();
  });
});

describe('the invitation role line', () => {
  const summary: InvitationRoleSummary = { role: 'viewer', start_topics: false, start_threads: false, send_inputs: false, answer_requests: false, model_access_required: true };
  it('keeps provider/model use separate from default page acts', () => {
    const view = show(<RoleSummary summary={summary} />);
    expect(screen.getByText('Under this org\'s current defaults: view work.')).toBeVisible();
    expect(screen.getByText('Sending also needs their own access to the provider and model.')).toBeVisible();
    view.rerender(<RoleSummary summary={{ ...summary, send_inputs: true, answer_requests: true }} />);
    expect(screen.getByText(/send to agents, answer requests/)).toBeVisible();
    expect(screen.getByText('Individual pages can differ.')).toBeVisible();
  });
});

describe('Start here', () => {
  it('has no card or inaccessible title for a missing guide', () => {
    show(<StartHere document={null} />);
    expect(screen.queryByRole('region', { name: 'Start here' })).not.toBeInTheDocument();
  });
});
