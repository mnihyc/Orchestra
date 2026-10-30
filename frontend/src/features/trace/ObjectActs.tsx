/** Deleted, retained and purged facts beside the explicit content and restoration acts. */
import { useMutation } from '@tanstack/react-query';
import { RotateCcw } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { restoreDocument } from '@/api/documents';
import { restoreNode } from '@/api/nodes';
import { useFormatContext } from '@/lib/appearance';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { toast } from '@/ui/Toast';
import { objectActs } from './objects';
import { EnvironmentRestore } from './EnvironmentRestore';
import type { AuditEntry as TraceEntry } from '@/generated/api-types';

export function ObjectActs({ object, onRestore, trace = false }: { object: TraceEntry['object']; onRestore?: () => void; trace?: boolean }) {
  const { t } = useTranslation(); const format = useFormatContext();
  const date = (day: string | null | undefined) => day ? new Intl.DateTimeFormat(format.locale, { dateStyle: 'medium', timeZone: format.timeZone }).format(new Date(day)) : t('trace.unknown');
  const acts = objectActs(object);
  const [confirming, setConfirming] = useState(false);
  const restorable = acts.restore && (!trace || onRestore !== undefined);
  const restore = useMutation({ mutationFn: async (): Promise<void> => { if (object.id === null) throw new Error('no object was named'); if (object.kind === 'document') await restoreDocument(object.id); else await restoreNode(object.id); }, onSuccess: () => { toast({ tone: 'success', title: t('trace.restored', { name: object.name ?? t('trace.unknown') }) }); } });
  if (object.state === 'purged') return <div className="flex flex-col gap-1 text-sm"><span className="font-medium wrap-anywhere">{object.name ?? t('trace.unknown')}</span><span className="text-text-muted">{t('trace.purged', { day: date(object.purged_at) })}</span></div>;
  return <div className="flex min-w-0 flex-col gap-2">
    {object.state === 'deleted' && <><Badge tone="warning" className="self-start">{t('trace.deleted')}</Badge><p className="text-sm wrap-anywhere text-text-muted">{object.deleted_by ? t('trace.deletedBy', { name: object.deleted_by.display_name, day: date(object.deleted_at), until: date(object.purge_at) }) : t('trace.deletedAnonymous', { day: date(object.deleted_at), until: date(object.purge_at) })}</p></>}
    <div className="flex flex-wrap gap-2">{acts.open && <Button size="sm" variant="ghost" action="open" asChild><a href={acts.open}>{t('trace.open')}</a></Button>}{acts.inspect && <Button size="sm" action="show" asChild><a href={acts.inspect}>{t('trace.inspect')}</a></Button>}{restorable && (trace || object.kind !== 'environment') && <Button size="sm" icon={<RotateCcw aria-hidden />} pending={restore.isPending} onClick={() => onRestore ? onRestore() : setConfirming(true)}>{t('trace.restore')}</Button>}</div>
    {!trace && object.kind === 'environment' && object.id !== null && <EnvironmentRestore id={object.id} allowed={restorable} />}{trace && acts.restore && !restorable && <p className="text-xs text-text-muted">{t('trace.eventCannotRestore')}</p>}{object.state === 'deleted' && !acts.inspect && !acts.restore && <p className="text-xs text-text-muted">{t('trace.contentGuard')}</p>}{restore.error !== null && <ErrorAlert error={restore.error} />}
    {confirming && <Dialog open title={t('trace.restoreTitle')} dismissible={!restore.isPending} onOpenChange={(open) => !open && !restore.isPending && setConfirming(false)} footer={<><Button disabled={restore.isPending} onClick={() => setConfirming(false)}>{t('common.cancel')}</Button><Button variant="primary" icon={<RotateCcw aria-hidden />} pending={restore.isPending} onClick={() => restore.mutate(undefined, { onSuccess: () => setConfirming(false) })}>{t('trace.bringBack')}</Button></>}><p className="py-3 wrap-anywhere">{object.name ?? t('trace.unknown')}</p><p>{t(object.kind === 'document' ? 'trace.returnsDocument' : 'trace.returnsArchived')}</p>{restore.error !== null && <ErrorAlert error={restore.error} />}</Dialog>}
  </div>;
}
