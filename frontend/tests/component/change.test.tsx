/** What a person sees while a change is made, drawn with no server: what stops a plan and what would lift it, whom to ask when access is missing, who may go on talking to the thread and the one tick that opens the provider to them, the bar of a job and its phases, each of the ways a job ends, the four steps and the one that is under way, and an editor that takes the keyboard into it when it opens. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { Blockers } from '@/features/change/Blockers';
import { Depends } from '@/features/change/Depends';
import { Steps } from '@/features/change/Frame';
import { JobProgress } from '@/features/change/Progress';
import { Result } from '@/features/change/Result';
import { Chip, Editor } from '@/features/change/Sentence';
import type { Dependency, Job, PlanBlocker } from '@/generated/api-types';
import { i18n } from '@/i18n';

import { show } from './support';

const GRACE = { id: 'u-grace', kind: 'human', username: 'grace', display_name: 'Grace Hopper', avatar: { text: 'G', color: 2 } };

const job = (more: Record<string, unknown> = {}): Job =>
  ({
    id: 'job-1',
    state: 'running',
    error: null,
    log: null,
    progress: { files_done: 0, files_total: null, rate_bps: null, percent: 45, phase: 'history', message: null, bytes_done: 0, bytes_total: null, eta_seconds: null, phases: [{ name: 'lock', state: 'done' }, { name: 'history', state: 'running' }, { name: 'verify', state: 'pending' }, { name: 'commit', state: 'pending' }] },
    ...more,
  }) as unknown as Job;

describe('what stops a plan', () => {
  it('says what is wrong and what would lift it, and offers the progress of the job that holds the thing', async () => {
    const onShow = vi.fn();
    const blockers: PlanBlocker[] = [{ code: 'already_running', message: 'a switch is under way' }];
    show(<Blockers blockers={blockers} plan={null} onShow={onShow} />);
    expect(screen.getByText('Another fork, switch, move or update is already under way here')).toBeVisible();
    expect(screen.getByText('Wait for it to end, or look at how far it has come.')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Show progress' }));
    expect(onShow).toHaveBeenCalledOnce();
  });

  it('names what is missing and whom to ask where access is, and offers no way to show a job that is not there', () => {
    const blockers = [
      {
        code: 'access',
        message: 'you may not',
        missing: [{ ask: [GRACE], ask_more: 2, override: false, closed: null, object: { id: 'p-1', kind: 'provider', name: 'Team Anthropic', owner: GRACE }, options: null, permissions: ['provider.use'] }],
      },
    ] as unknown as PlanBlocker[];
    show(<Blockers blockers={blockers} plan={null} />);
    expect(screen.getByText('You may not do this yet')).toBeVisible();
    expect(screen.getByText('Ask the people named below, or choose something you may use.')).toBeVisible();
    expect(screen.getByText('You can’t use Team Anthropic.')).toBeVisible();
    expect(screen.getByText('Ask Grace Hopper, or one of 2 others.')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Show progress' })).toBeNull();
  });

  it('draws nothing where nothing stands in the way', () => {
    const { container } = show(<Blockers blockers={[]} plan={null} />);
    expect(container.querySelector('[data-blockers]')).toBeNull();
  });
});

describe('who may go on talking to the thread', () => {
  const dependency = (more: Record<string, unknown>): Dependency =>
    ({ can_give: true, for: 'thread.interact', object: { id: 'prov-2', kind: 'provider', name: 'Team Anthropic', owner: GRACE }, open: 'some', options: null, permission: 'provider.use', through: { id: 'node-1', kind: 'project', name: 'Infra', redacted: false }, ...more }) as unknown as Dependency;

  it('says nothing to do where everybody who can talk to it has the provider already', () => {
    show(<Depends depends={[dependency({ open: 'all' })]} model="gpt-5.4" ticked={new Set()} onTick={vi.fn()} />);
    expect(screen.getByText('Talking to it will need gpt-5.4 on Team Anthropic. Everyone who can talk to it has that.')).toBeVisible();
    expect(screen.queryByRole('checkbox')).toBeNull();
  });

  it('offers whoever may give the use of the provider one tick, which opens it to the work in the place the plan names', async () => {
    const user = userEvent.setup();
    const onTick = vi.fn();
    const { container, rerender } = show(<Depends depends={[dependency({})]} model="gpt-5.4" ticked={new Set()} onTick={onTick} />);
    expect(screen.getByText('Some of the people who can talk to it may not use Team Anthropic.')).toBeVisible();
    expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
    await user.click(screen.getByRole('checkbox', { name: 'Open Team Anthropic to the work in Infra' }));
    expect(onTick).toHaveBeenCalledWith('prov-2', true);
    rerender(<Depends depends={[dependency({})]} model="gpt-5.4" ticked={new Set(['prov-2'])} onTick={onTick} />);
    expect(screen.getByRole('checkbox', { name: 'Open Team Anthropic to the work in Infra' })).toBeChecked();
    expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
    const choice = screen.getByRole('checkbox', { name: 'Open Team Anthropic to the work in Infra' });
    await user.unhover(choice);
    await user.hover(choice);
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Gives them the use of Team Anthropic. It changes nothing else.');
    expect(choice).toHaveAccessibleDescription('Gives them the use of Team Anthropic. It changes nothing else.');
  });

  it('says who decides, and that they can ask from the thread, to whoever may not give it', () => {
    show(<Depends depends={[dependency({ can_give: false, open: 'none' })]} model="gpt-5.4" ticked={new Set()} onTick={vi.fn()} />);
    expect(screen.getByText('None of the people who can talk to it may use Team Anthropic.')).toBeVisible();
    expect(screen.getByText('Grace Hopper decides that: they can ask from the thread.')).toBeVisible();
    expect(screen.queryByRole('checkbox')).toBeNull();
  });
});

describe('a job under way', () => {
  it('says what it is, how far it has come in numbers and in the phase it is in, and marks the phase that is under way by more than its colour', () => {
    show(<JobProgress job={job()} heading="Switching to Team Anthropic" reconnecting={false} />);
    expect(screen.getByRole('heading', { name: 'Switching to Team Anthropic' })).toBeVisible();
    expect(screen.getByText('45 %')).toBeVisible();
    expect(screen.getByText('Carrying the conversation over')).toBeVisible();
    const phases = within(screen.getByRole('list', { name: 'Phases' })).getAllByRole('listitem');
    expect(phases.map((item) => item.getAttribute('data-phase-state'))).toEqual(['done', 'running', 'pending', 'pending']);
    expect(phases[1]).toHaveAttribute('aria-current', 'step');
    expect(phases[1]).toHaveTextContent('under way');
  });

  it('says that it is waiting for its turn before it has begun, and that it is looking again when the line is lost', () => {
    show(<JobProgress job={job({ state: 'queued' })} heading="Switching" reconnecting />);
    expect(screen.getByText('Waiting for its turn')).toBeVisible();
    expect(screen.getByText('Reconnecting…')).toBeVisible();
    expect(screen.getByText('The job goes on without this window. The bar stands where it was last seen.')).toBeVisible();
  });

  it('shows the bytes copied and the time left while files are copied, and the checks of the new side as they pass', () => {
    const copying = job({ progress: { files_done: 0, files_total: null, rate_bps: null, percent: 30, phase: 'copy', message: null, bytes_done: 500_000_000, bytes_total: 1_000_000_000, eta_seconds: 120, phases: [{ name: 'copy', state: 'running' }] } });
    show(<JobProgress job={copying} heading="Forking" reconnecting={false} />);
    expect(screen.getByText('Copying · 0 files · 477 MiB of 954 MiB · ~2 min left')).toBeVisible();
  });

  it('lists the checks of the new side only while it is being checked, and only those the job printed', () => {
    const checking = job({ log: 'environment attached\nfiles present', progress: { files_done: 0, files_total: null, rate_bps: null, percent: 80, phase: 'verify', message: null, bytes_done: 0, bytes_total: null, eta_seconds: null, phases: [{ name: 'verify', state: 'running' }] } });
    show(<JobProgress job={checking} heading="Switching" reconnecting={false} />);
    expect(within(screen.getByRole('list', { name: 'Checks of the new side' })).getAllByRole('listitem').map((item) => item.textContent.trim())).toEqual(['Environment attached', 'Files present']);
  });
});

describe('the end of a job', () => {
  it('names a non-retryable approval refusal and the action that can fix it', () => {
    const failed = job({ state: 'failed', error: { code: 'approval_mode_unsupported', message: 'unsupported', phase: 'history', undone: true } });
    show(<Result job={failed} done="" stoppedTurn={false} waited={10} subject="Main" />);
    expect(screen.getByRole('alert')).toHaveTextContent('The target program no longer offers the planned approval mode. Review the switch and choose a compatible program.');
    expect(screen.getByRole('alert')).not.toHaveTextContent('Try again');
  });

  it('says that it worked, what was checked, and what the thread now runs on', () => {
    show(<Result job={job({ state: 'succeeded', log: 'environment attached\nmodel usable' })} done="This thread now runs on Team Anthropic, with claude-sonnet-5." stoppedTurn={false} waited={10} subject="Main" />);
    expect(screen.queryByText('Done')).toBeNull();
    expect(screen.getByText('This thread now runs on Team Anthropic, with claude-sonnet-5.')).toBeVisible();
    expect(screen.getByText('Checked: Environment attached, Model usable')).toBeVisible();
  });

  it('says the catalogued failure and complete rollback once, in one alert', () => {
    const failed = job({ state: 'failed', error: { code: 'verify_failed', message: 'the tools could not reach it', phase: 'verify', undone: true } });
    show(<Result job={failed} done="" stoppedTurn={false} waited={10} subject="Main thread" />);
    // The safe cause is the status, without a second generic failed-phase heading.
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    const alert = screen.getByRole('alert');
    expect(within(alert).getByText(i18n.t('change.failure.verify_failed', { message: i18n.t('ui.error.operationFailed') }))).toBeVisible();
    expect(within(alert).getByText('Everything it had made was removed. Main thread is as it was before.')).toBeVisible();
    expect(screen.queryByText(/^It did not work/)).toBeNull();
    expect(alert).not.toHaveTextContent('the tools could not reach it');
    expect(alert).not.toHaveTextContent('verify_failed');
  });

  it('says that some of what it made may still be there when it could not take it all back', () => {
    const failed = job({ state: 'failed', error: { code: 'environment_unavailable', message: 'the host went away', phase: 'provision', undone: false } });
    show(<Result job={failed} done="" stoppedTurn={false} waited={10} subject="Main thread" />);
    expect(screen.getByText('Some of what it made may still be there. The system is trying to remove it.')).toBeVisible();
    expect(screen.queryByText(/is as it was before/)).toBeNull();
  });

  it('says how long it waited for a turn that did not end, and that a turn that was stopped was stopped as asked', () => {
    const waited = job({ state: 'failed', error: { code: 'wait_timeout', message: 'x', phase: 'lock', undone: true } });
    show(<Result job={waited} done="" stoppedTurn waited={7} subject="Main" />);
    expect(screen.getByText('The turn was still running after 7 minutes. Nothing was changed.')).toBeVisible();
    expect(screen.getByText('The turn that was running was stopped, as you asked.')).toBeVisible();
  });

  it('says that it was cancelled, and that what it had made is gone', () => {
    show(<Result job={job({ state: 'cancelled' })} done="" stoppedTurn={false} waited={10} subject="Sandbox" />);
    expect(screen.getByText('Cancelled')).toBeVisible();
    expect(screen.getByText('Everything it had made was removed. Sandbox is as it was before.')).toBeVisible();
  });
});

describe('the four steps', () => {
  it('mark the one that is under way by more than their colour, and the ones before it as over', () => {
    show(<Steps at="progress" />);
    const steps = within(screen.getByRole('list', { name: 'Steps' })).getAllByRole('listitem');
    expect(steps.map((step) => step.textContent)).toEqual(['1Plan—', '2Confirm—', '3Progress—', '4Result'].map((text, n) => (n < 2 ? text.replace(/^\d/, '') : text)));
    expect(steps.map((step) => step.getAttribute('aria-current'))).toEqual([null, null, 'step', null]);
  });
});

describe('a choice of the sentence and the editor it opens', () => {
  it('says by its state that its editor is open, and by its look that what it holds is not what the thread has now', async () => {
    const onToggle = vi.fn();
    show(
      <Chip label="Provider and model" open={false} changed controls="editor" onToggle={onToggle}>
        OpenAI key · gpt-5.4
      </Chip>,
    );
    const chip = screen.getByRole('button', { name: 'Provider and model' });
    expect(chip).toHaveAttribute('aria-expanded', 'false');
    expect(chip).toHaveAttribute('data-changed', 'true');
    await userEvent.click(chip);
    expect(onToggle).toHaveBeenCalledOnce();
  });

  it('takes the keyboard into its first control when it opens, the choice that is made where there is a group', () => {
    show(
      <Editor id="editor" title="History" about="How much">
        <button type="button">Before</button>
        <div role="radiogroup" aria-label="History">
          <button role="radio" aria-checked="false" type="button">
            All
          </button>
          <button role="radio" aria-checked="true" type="button">
            Some
          </button>
        </div>
      </Editor>,
    );
    expect(screen.getByRole('radio', { name: 'Some' })).toHaveFocus();
  });

  it('takes the keyboard into the first control it has where there is no group', () => {
    show(
      <Editor id="editor" title="Environment" about="Where">
        <a href="#top">A link is no control of a form</a>
        <input aria-label="Name" />
      </Editor>,
    );
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveFocus();
  });

  it('puts the hint of its heading beside the title', () => {
    show(
      <Editor id="editor" title="History" about="How much" hint={<span>Hint here</span>}>
        <input aria-label="x" />
      </Editor>,
    );
    expect(within(screen.getByRole('region', { name: 'History' })).getByText('Hint here')).toBeVisible();
  });
});
