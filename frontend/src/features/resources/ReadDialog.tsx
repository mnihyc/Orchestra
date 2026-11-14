/** A list act reads the full resource only after the person opens its dialog. */
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows, type SkeletonRead } from '@/ui/Skeleton';

export function ResourceReadDialog({ name, read, onClose }: { name: string; read: SkeletonRead; onClose: () => void }) {
  return <Dialog open title={name} onOpenChange={(open) => { if (!open) onClose(); }}>
    {read.isError === true ? <ErrorState compact error={read.error} onRetry={() => { void read.refetch?.(); }} /> : <SkeletonRows read={read} rows={3} />}
  </Dialog>;
}
