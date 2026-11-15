/** A private seed/history choice pins one root for its page, then ordinary page following resumes. */
import { useState } from 'react';
import type { Org } from '@/generated/api-types';

/**
 * The organization the page in view asks in: its own; for a page of no organization (Management, Settings, the
 * inbox) the one the rail shows when it is one of the person's, else their personal one. The rail keeps an
 * organization reached through As admin or through sharing for as long as the page stays open, and Ask answers
 * only as a member, so following it there would refuse every question. Before the person's organizations
 * arrive the rail's choice is kept.
 */
export function askPageRoot(
  page: { own: string | null | undefined; heading: string | null | undefined },
  selected: string | null,
  orgs: readonly Pick<Org, 'id' | 'org_id' | 'org_kind'>[],
): string | null {
  const shown = orgs.find((org) => org.id === selected);
  return page.own ?? page.heading ?? shown?.org_id ?? (orgs.length === 0 ? selected : null) ?? orgs.find((org) => org.org_kind === 'personal')?.id ?? null;
}

interface RootSeed { readonly id: string; readonly root: string | null }
interface Selection { pageRoot: string | null; seedId: string | null; root: string | null }

export function useAskRoot(pageRoot: string | null, seed: RootSeed | null): readonly [string | null, (root: string) => void] {
  const seedId = seed?.id ?? null;
  const [selection, setSelection] = useState<Selection>(() => ({ pageRoot, seedId, root: seed?.root ?? null }));
  if (selection.pageRoot !== pageRoot || selection.seedId !== seedId) {
    setSelection({ pageRoot, seedId, root: seed?.root ?? (selection.pageRoot === pageRoot ? selection.root : null) });
  }
  const root = seed?.root ?? (selection.pageRoot === pageRoot ? selection.root : null) ?? pageRoot;
  const choose = (chosen: string) => setSelection({ pageRoot, seedId, root: chosen });
  return [root, choose];
}
