/** A private seed/history choice pins one root for its page, then ordinary page following resumes. */
import { useState } from 'react';

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
