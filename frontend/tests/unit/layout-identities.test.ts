/** A restored layout must not give a newly addressed page an existing tab's identity. */
import { afterEach, expect, it, vi } from 'vitest';

afterEach(() => {
  vi.useRealTimers();
  vi.resetModules();
});

it('keeps new layout identities distinct from a previous page at the same clock instant', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-27T12:00:05.000Z'));
  vi.resetModules();
  const previous = await import('../../src/features/workbench/layout');
  const saved = previous.makeGroup([previous.makeTab('page', { href: '/org/acme' })]);

  // A hard reload starts a fresh module instance while the stored layout survives.
  vi.resetModules();
  const reloaded = await import('../../src/features/workbench/layout');
  const addressed = reloaded.makeGroup([reloaded.makeTab('page', { href: '/resources/hosts/new-host' })]);

  expect(addressed.id).not.toBe(saved.id);
  expect(addressed.tabs[0]?.id).not.toBe(saved.tabs[0]?.id);
  expect(addressed.tabs[0]?.of).toEqual({ href: '/resources/hosts/new-host' });
});
