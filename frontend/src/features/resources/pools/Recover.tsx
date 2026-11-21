/** A manager releases this pool's suppression; the next real request owns any native trial. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { readPoolAfterWrite, recoverPool } from '@/api/pools';
import type { Pool } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { adminSpec } from '@/ui/AdminAct';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

export function PoolRecover({ pool }: { pool: Pool }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const key = useActKey();
  const written = useRef(false);
  const [accepted, setAccepted] = useState(false);
  const recover = useMutation({ mutationFn: async () => {
    if (!written.current) {
      const body = { expected_revision: pool.revision };
      await recoverPool(pool.id, pool.revision, key(body));
      written.current = true; setAccepted(true);
    }
    await readPoolAfterWrite(cache, pool.id);
  }, onSuccess: () => { written.current = false; setAccepted(false); } });
  return <div className="flex flex-col items-start gap-2">
    <Button size="sm" admin={adminSpec(pool, 'pool.manage')} disabled={!holds(pool.access, 'pool.manage')} pending={recover.isPending}
      onClick={() => recover.mutate()}>{t(accepted ? 'pools.readAgain' : 'pools.recover')}</Button>
    {recover.error !== null && <ErrorAlert error={recover.error} />}
  </div>;
}
