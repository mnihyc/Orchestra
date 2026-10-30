/** An org's UUID alias is resolved once and gives home/settings the same native canonical record. */
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { nodeQuery, orgBySlugQuery } from '@/api/nodes';
import { queryClient } from '@/api/query';
import { canonicalOrgAddress } from '@/features/navigation/orgAddress';
import type { Project } from '@/generated/api-types';
import { me, org, userRef } from '../doubles/fixtures';

const department = org('00000000-0000-4000-8000-000000000001', 'research', 'Research', userRef(me(1, 'ada', 'Ada')), { address: 'acme.research' });
afterEach(() => { vi.restoreAllMocks(); queryClient.clear(); });

it('uses the native identifier lookup and seeds the canonical Org for the redirected screen', async () => {
  const read = vi.spyOn(queryClient, 'query').mockResolvedValue(department);
  expect(await canonicalOrgAddress(department.id)).toBe('acme.research');
  expect(read).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ queryKey: nodeQuery(department.id).queryKey }));
  expect(queryClient.getQueryData(orgBySlugQuery(department.address).queryKey)).toEqual(department);
});

it('leaves ordinary addresses to their own reader and never treats a project as an org', async () => {
  const project: Project = { ...department, kind: 'project', topic_count: 0 };
  const read = vi.spyOn(queryClient, 'query').mockResolvedValue(project);
  expect(await canonicalOrgAddress('acme.research')).toBeUndefined(); expect(read).not.toHaveBeenCalled();
  expect(await canonicalOrgAddress(project.id)).toBeUndefined();
  expect(queryClient.getQueryData(orgBySlugQuery(department.address).queryKey)).toBeUndefined();
});

it('retains the native refusal instead of guessing or retrying an address', async () => {
  const error = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Not allowed', detail: 'Org View needed', status: 403, request_id: '' }, null);
  const read = vi.spyOn(queryClient, 'query').mockRejectedValue(error);
  await expect(canonicalOrgAddress(department.id)).rejects.toBe(error);
  expect(read).toHaveBeenCalledTimes(1);
  expect(queryClient.getQueryData(orgBySlugQuery(department.address).queryKey)).toBeUndefined();
});
