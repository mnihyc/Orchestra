/** Moving a pool uses the ordinary native home writer and keeps its owner and written rules. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api, ok, repeatable } from '@/api/client';
import { readPoolAfterWrite } from '@/api/pools';
import type { Pool } from '@/generated/api-types';
import { adminSpec } from '@/ui/AdminAct';
import { EditDialog } from '@/features/resources/EditDialog';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import { useActKey } from '@/lib/forms';

export function PoolHomeDialog({ pool, onClose }: { pool: Pool; onClose: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const key = useActKey();
  const written = useRef(false);
  const [accepted, setAccepted] = useState(false);
  const [home, setHome] = useState<PickedPlace | null>(null);
  const valid = home !== null && (home.kind === 'org' || home.kind === 'project') && home.id !== pool.home?.id;
  const save = useMutation({ mutationFn: async () => {
    if (!written.current) {
      if (!valid) throw new Error(t('pools.form.invalid'));
      const body = { home_id: home.id, expected_revision: pool.revision };
      await ok(api.PUT('/{object}/{id}/home', { params: { path: { object: 'pools', id: pool.id } }, body, ...repeatable(key(body)) }));
      written.current = true; setAccepted(true);
    }
    await readPoolAfterWrite(cache, pool.id);
  }, onSuccess: onClose });
  return <EditDialog admin={adminSpec(pool, 'pool.manage')} title={t('pools.moveHome')} submitLabel={accepted ? t('pools.readAgain') : undefined} changed valid={valid}
    pending={save.isPending} error={save.error} onClose={onClose} onSave={() => save.mutate()}>
    <PlacePicker label={t('pools.home')} value={home} onChange={setHome} disabled={save.isPending || accepted}
      refusal={(place) => place.kind === 'org' || place.kind === 'project' ? false : t('pools.homeRequired')} />
  </EditDialog>;
}
