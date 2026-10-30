/** Changing a page: the one request that edits it, made against the revision the person saw, with the page somebody else changed meanwhile shown and never overwritten.
 *
 * Every change of a page, of a defaults page or of its owners goes through here. It is asked of the
 * page as it stands when the person acts, carries that page's revision, and is applied as a whole or
 * not at all. While it is under way the controls that change the page wait (a slow answer is not
 * pressed twice), and nothing is shown as done before the answer. When the page moved on meanwhile,
 * the answer carries the page as it is now: that page is shown in place of the one the person saw, the
 * change is not made, and the person is told what they did and can make it again against the new page
 * or leave it.
 */
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { queryOptions, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';

import {
  type AccessTarget,
  type AccessRowTarget,
  type SystemAccessTarget,
  changeSystemDefaults,
  previewSystemDefaults,
  readSystemDefaults,
  systemDefaultsKey,
  changeDefaults,
  changePage,
  currentPageOf,
  defaultsKey,
  pageKey,
  previewDefaults,
  previewPage,
  readDefaults,
  readPage,
} from '@/api/access';
import { ApiError } from '@/api/client';
import type { PageChange, PagePreview, PermissionPage } from '@/generated/api-types';

/** What a page is: the page of a thing, or what a place gives to what is made inside it. */
export type PageSource = { type: 'page'; target: AccessTarget } | { type: 'defaults'; place: AccessTarget } | { type: 'system-defaults'; place: SystemAccessTarget };

export const sourceObject = (source: PageSource): AccessRowTarget => (source.type === 'page' ? source.target : source.place);

export const sourceKey = (source: PageSource) => source.type === 'page' ? pageKey(source.target) : source.type === 'system-defaults' ? systemDefaultsKey : defaultsKey(source.place.id);

/** The page of a source, kept: one shape whichever it is, so that a component reads either alike. */
export const sourceQuery = (source: PageSource) =>
  queryOptions({
    ...(source.type === 'system-defaults' ? ADMIN_QUERY : {}),
    queryKey: sourceKey(source),
    queryFn: ({ signal }): Promise<PermissionPage> => source.type === 'page' ? readPage(source.target, signal) : source.type === 'system-defaults' ? readSystemDefaults(signal) : readDefaults(source.place.id, signal),
  });

const sourceChange = (source: PageSource, change: PageChange): Promise<PermissionPage> =>
  source.type === 'page' ? changePage(source.target, change) : source.type === 'system-defaults' ? changeSystemDefaults(change) : changeDefaults(source.place.id, change);

export const sourcePreview = (source: PageSource, change: PageChange): Promise<PagePreview> =>
  source.type === 'page' ? previewPage(source.target, change) : source.type === 'system-defaults' ? previewSystemDefaults(change) : previewDefaults(source.place.id, change);

/** What was done to the page, in words, and how to do it again against a newer page. */
export interface Request {
  label: string;
  make: (page: PermissionPage) => PageChange;
}

/** A change the server did not make because the page had moved on. */
export interface Stale extends Request {
  /** The page as it is now, which is what is shown. */
  current: PermissionPage | null;
}

export interface Changes {
  /** Applies the change; answers whether it was made. A refusal is in `error` and a moved page in `stale`. */
  run: (request: Request) => Promise<boolean>;
  /** A change is under way: the controls that change the page wait. */
  busy: boolean;
  /** What is being done, in words. */
  doing: string | null;
  /** What the server refused the last change with, other than a page that moved on. */
  error: unknown;
  /** The last change that did not apply because the page had moved on. */
  stale: Stale | null;
  /** Makes the stale change again against the page as it is now. */
  again: () => Promise<boolean>;
  /** Puts the notice and the refusal away. */
  forget: () => void;
}

export function useChanges(source: PageSource): Changes {
  const queryClient = useQueryClient();
  const key = sourceKey(source);
  const [stale, setStale] = useState<Stale | null>(null);
  const [doing, setDoing] = useState<string | null>(null);

  const mutation = useMutation({
    mutationFn: async (request: Request): Promise<PermissionPage> => {
      // The page as it stands now, not as the screen drew it a minute ago.
      const page = queryClient.getQueryData<PermissionPage>(key) ?? (await queryClient.query(sourceQuery(source)));
      return sourceChange(source, request.make(page));
    },
    onMutate: (request) => {
      setDoing(request.label);
      setStale(null);
    },
    onError: (error, request) => {
      const current = currentPageOf(error);
      if (current !== null) queryClient.setQueryData(key, current);
      // A moved page is shown as it is now; the change that was meant for the old one waits to be made again or left.
      if (current !== null || (error instanceof ApiError && error.is('conflict.stale_revision'))) {
        setStale({ ...request, current });
        if (current === null) void queryClient.invalidateQueries({ queryKey: key });
      }
    },
    onSettled: () => setDoing(null),
  });

  const run = async (request: Request): Promise<boolean> => {
    try {
      await mutation.mutateAsync(request);
      return true;
    } catch {
      return false;
    }
  };

  return {
    run,
    busy: mutation.isPending,
    doing,
    error: stale === null ? mutation.error : null,
    stale,
    again: () => (stale === null ? Promise.resolve(false) : run(stale)),
    forget: () => {
      setStale(null);
      mutation.reset();
    },
  };
}

/** The page of a thing or a place, read once and kept as the changes leave it. */
export function usePageSource(source: PageSource) {
  return useQuery(sourceQuery(source));
}
