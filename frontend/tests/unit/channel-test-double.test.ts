// @vitest-environment node
/** A routed test cannot invent acceptance when its channel's own notifier reported failure. */
import { expect, it } from 'vitest';
import type { NotificationChannel } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

it('preserves the native org-channel test failure through the route adapter', async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const made = world.settings.handle(person, 'POST', `/orgs/${ACME.org}/notification-channels`, new URLSearchParams(), {
    kind: 'webhook', name: 'Failing hook', config: { url: 'https://hooks.example/failing/ops' },
  })!.body as NotificationChannel;
  const path = `/notification-channels/${made.id}/test`;
  const native = world.settings.handle(person, 'POST', path, new URLSearchParams(), {});
  expect(native?.body).toMatchObject({ ok: false, error: 'HTTP 502 Bad Gateway from hooks.example: upstream timed out after 10s' });
  expect(await world.away.handle(person, 'POST', path, new URLSearchParams(), {})).toEqual(native);
});
