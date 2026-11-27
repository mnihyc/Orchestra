/** A default that cannot work for the person is said before the act: "Default host" comes to a host only where the person may use one
 * (a provider's new container only on a host of theirs or nobody's), an environment that failed says why it could not start, a
 * switch that failed names what was not available in one sentence, and a first making is called a making, not an update. */
import { expect, it } from 'vitest';
import type { Host, HostDriver, Job, Thread } from '@/generated/api-types';
import { automaticHostWorks } from '@/features/start/NewEnvironment';
import { failedEnvironmentWhy, waitingWords } from '@/features/timeline/waiting';
import { reasonWords } from '@/features/change/Result';
import { provisionKind } from '@/features/change/EnvironmentChange';
import { environmentPlaceWhy } from '@/features/start/words';
import { hostUnavailable } from '@/features/resources/choiceAvailability';
import { i18n } from '@/i18n';
import { enrollmentHost } from '../doubles/enrollment-row';
import { failedStart } from '../component/failed-start-fixtures';
import { host as placeHost } from './start-fixtures';

const t = i18n.getFixedT('en');

const docker: HostDriver = { name: 'docker', title: 'Docker', creates: ['environment', 'provider'], runtime_classes: ['runc'] };
function host(owner: string | null, permissions: string[]): Host {
  return { ...enrollmentHost(`h-${owner ?? 'system'}`, true), drivers: [docker], owner: owner === null ? null : { id: owner, display_name: owner, username: owner } as Host['owner'],
    access: { permissions, administers: false, through: [], admin_only: [], participating: true } };
}
const refusal = (kind: 'environment' | 'provider') => (entry: Host) => hostUnavailable(entry, t, kind);

it('comes to a host for a new environment only where the person may use one', () => {
  expect(automaticHostWorks([], 'environment', 'me', refusal('environment'))).toBe(false);
  expect(automaticHostWorks([placeHost('h1', 'orch-node', { usable: false })], 'environment', 'me', refusal('environment'))).toBe(false);
  expect(automaticHostWorks([placeHost('h1', 'orch-node', { usable: false }), placeHost('h2', 'mine')], 'environment', 'me', refusal('environment'))).toBe(true);
});

it("places a provider's new container by itself only on a host of its owner's or one nobody owns", () => {
  const use = ['host.view', 'host.use'];
  // Another person's host, even one the person may use (an administrator reaches the installer's bundled host), is taken only when named.
  expect(automaticHostWorks([host('ada', use)], 'provider', 'me', refusal('provider'))).toBe(false);
  expect(automaticHostWorks([host('me', use)], 'provider', 'me', refusal('provider'))).toBe(true);
  expect(automaticHostWorks([host(null, use)], 'provider', 'me', refusal('provider'))).toBe(true);
  expect(automaticHostWorks([host('me', ['host.view'])], 'provider', 'me', refusal('provider')), 'a host the person may not use').toBe(false);
});

it('says why an environment failed without the step it stopped in or a second stop', () => {
  expect(failedEnvironmentWhy('the environment failed: schedule: the host that was named is not one its creator may use'))
    .toBe('the host that was named is not one its creator may use');
  expect(failedEnvironmentWhy('the environment failed: its creator may use no host.')).toBe('its creator may use no host');
  expect(failedEnvironmentWhy('the environment failed: ')).toBeNull();
  expect(failedEnvironmentWhy('the environment is offline')).toBeNull();
});

it('gives the reason on the line of a thread its failed environment blocks', () => {
  const failed = failedStart().thread;
  if (failed.blocked === null) throw new Error('the fixture is blocked');
  const thread: Thread = { ...failed, blocked: { ...failed.blocked, detail: 'the environment failed: schedule: its creator may use no host' } };
  expect(waitingWords(thread, t)).toBe(`${t('startRecovery.failedWhy', { environment: 'Main', why: 'its creator may use no host' })} ${t('startRecovery.held')}`);
  expect(waitingWords(thread, t)).toBe('Main could not start: its creator may use no host. Your message is kept.');
});

function failedJob(code: string, message: string): Job {
  const { job } = failedStart();
  return { ...job, kind: 'thread_fork', state: 'failed', error: { code, message, phase: 'copy', undone: true } };
}

it('says what was not available in one sentence with one stop', () => {
  // The API serves a job's reason by its code; the page's sentence takes those words without their own stop.
  const reason = reasonWords(failedJob('environment_unavailable', 'The environment is not ready at the moment. Try again shortly.'), t, null);
  expect(reason).toBe('The environment was not available: The environment is not ready at the moment. Try again shortly.');
  expect(reason).not.toMatch(/\.\.$/u);
  expect(reasonWords(failedJob('binding_incompatible', 'the agent speaks no protocol the provider offers.'), t, null))
    .toBe('The agent and the provider do not fit together: the agent speaks no protocol the provider offers.');
});

it('calls the job that makes an environment a making and the one that starts it a start', () => {
  const phase = (name: string) => ({ name, state: 'pending', weight: 1 }) as Job['progress']['phases'][number];
  const progress = (names: string[]) => ({ progress: { ...failedStart().job.progress, phases: names.map(phase) } });
  expect(provisionKind(progress(['schedule', 'image', 'volumes', 'tools', 'create']))).toBe('make');
  expect(provisionKind(progress(['start', 'start_enroll', 'start_attach', 'files']))).toBe('start');
});

it('calls an environment that failed, was lost or was removed by its own state in the pickers that move a thread', () => {
  const place = (status: 'failed' | 'offline' | 'lost', reasons: ('offline' | 'removing')[]) =>
    ({ reasons, environment: { ...failedStart().thread.environment, status } }) as Parameters<typeof environmentPlaceWhy>[0];
  expect(environmentPlaceWhy(place('failed', ['offline']), t)).toBe('Failed');
  expect(environmentPlaceWhy(place('lost', ['offline']), t)).toBe('Lost');
  expect(environmentPlaceWhy(place('offline', ['offline']), t)).toBe(t('change.where.envReason.offline'));
  expect(environmentPlaceWhy(place('failed', ['offline', 'removing']), t)).toBe(`Failed${t('startFlow.separator')}${t('change.where.envReason.removing')}`);
});
