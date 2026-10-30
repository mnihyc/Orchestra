/** Cached private views still report an opening, without making another grant or content read. */
import { useContext, useEffect } from 'react';
import { api, done } from '@/api/client';
import { openingId } from '@/lib/opening';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';

export function useAdminOpening(kind: string, id: string | undefined, reached: boolean): void {
  const viewOpening = useContext(ViewOpeningContext);
  const nonce = viewOpening === undefined ? openingId() : viewOpening;
  useEffect(() => {
    if (id === undefined || !reached || nonce === null) return;
    // A lost answer is harmless: the server deduplicates this nonce against HTTP/live openings.
    void done(api.POST('/me/admin-openings', { body: { kind, id, opening_id: nonce }, headers: { 'x-orch-opening': nonce } })).catch(() => undefined);
  }, [kind, id, reached, nonce]);
}
