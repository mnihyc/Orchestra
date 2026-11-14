/** What changes only with a release, or with an act that says so, is read once for the life of the page. */
import { describe, expect, it } from 'vitest';

import { permissionsQuery } from '@/api/access';
import { notifiersQuery as notifierKinds } from '@/api/notifications';
import { capabilitiesQuery, driversQuery, harnessKindsQuery } from '@/api/resources';
import { notifiersQuery, registryQuery } from '@/api/settings';
import { settingRegistryQuery } from '@/api/search';

describe('reads that cannot change while the page stands', () => {
  it.each([
    ['the kinds of harness and their builds', harnessKindsQuery],
    ['the drivers', driversQuery],
    ['what an environment may be granted', capabilitiesQuery],
    ['the permission catalogue', permissionsQuery],
    ['the kinds of channel (as the shell lists them)', notifierKinds],
    ['the kinds of channel (as the settings list them)', notifiersQuery],
    ['the registry of settings', registryQuery],
    ['the registry of settings (as the finder reads it)', settingRegistryQuery],
  ])('keeps %s for the life of the page', (_name, query) => {
    expect(query.staleTime).toBe(Number.POSITIVE_INFINITY);
  });
});
