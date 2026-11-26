/** A pool's removal uses its native revision and leaves every shared member in place. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { applyRow } from '@/api/cache';
import { deletePool } from '@/api/pools';
import { ResourceLibrary } from '@/features/resources/Library';
import type { Pool } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { adminSpec } from '@/ui/AdminAct';
import { ConfirmDialog } from '@/ui/ConfirmDialog';

export function DeletePool({ pool, onClose, onDeleted }: { pool: Pool; onClose: () => void; onDeleted: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const key = useActKey();
  const remove = useMutation({ mutationFn: () => deletePool(pool.id, pool.revision, key({ id: pool.id, expected_revision: pool.revision })),
    onSuccess: () => { applyRow(cache, { kind: 'pool', id: pool.id, value: null }); onDeleted(); } });
  return <ConfirmDialog admin={adminSpec(pool, 'pool.manage')} open onOpenChange={(open) => { if (!open) onClose(); }} danger title={t('pools.deleteName', { name: pool.name })}
    confirmLabel={t('pools.deleteName', { name: pool.name })} pending={remove.isPending} error={remove.error} onConfirm={() => remove.mutate()}>
    <div className="flex flex-col gap-2"><span>{t('pools.deleteEffect')}</span><ResourceLibrary object={{kind:'pool',id:pool.id}} /></div>
  </ConfirmDialog>;
}
