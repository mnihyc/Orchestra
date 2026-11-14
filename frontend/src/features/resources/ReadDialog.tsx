/** A list act reads the full resource only after the person opens its dialog. */
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows, type SkeletonRead } from '@/ui/Skeleton';

/** A newer list row must not initialize a form from an older cached full view. */
export function currentResourceDetail<T extends { revision: number }>(detail: T | undefined, row: { revision: number } | null | undefined): T | undefined {
  return row !== null && row !== undefined && detail !== undefined && detail.revision >= row.revision ? detail : undefined;
}

export function ResourceReadDialog({ name, read, onClose }: { name: string; read: SkeletonRead; onClose: () => void }) {
  return <Dialog open title={name} onOpenChange={(open) => { if (!open) onClose(); }}>
    {read.isError === true ? <ErrorState compact error={read.error} onRetry={() => { void read.refetch?.(); }} /> : <SkeletonRows read={read} rows={3} />}
  </Dialog>;
}
