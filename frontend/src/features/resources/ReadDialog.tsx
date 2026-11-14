/** A list act reads the full resource only after the person opens its dialog. */
import { useEffect } from 'react';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows, type SkeletonRead } from '@/ui/Skeleton';

/** A newer list row must not initialize a form from an older cached full view. */
export function currentResourceDetail<T extends { revision: number }>(detail: T | undefined, row: { revision: number } | null | undefined): T | undefined {
  return row !== null && row !== undefined && detail !== undefined && detail.revision >= row.revision ? detail : undefined;
}

/** A response already in flight before the row changed must lead to a fresh read. */
export function useCurrentResourceDetail<T extends { revision: number }>(read: {
  data: T | undefined;
  isFetching: boolean;
  isError: boolean;
  refetch: (options?: { cancelRefetch?: boolean }) => Promise<unknown>;
}, row: { revision: number } | null | undefined): T | undefined {
  const received = read.data?.revision, wanted = row?.revision;
  const { isFetching, isError, refetch } = read;
  useEffect(() => {
    if (wanted !== undefined && received !== undefined && received < wanted && !isFetching && !isError) {
      void refetch({ cancelRefetch: false });
    }
  }, [received, wanted, isFetching, isError, refetch]);
  return currentResourceDetail(read.data, row);
}

export function ResourceReadDialog({ name, read, onClose }: { name: string; read: SkeletonRead; onClose: () => void }) {
  return <Dialog open title={name} onOpenChange={(open) => { if (!open) onClose(); }}>
    {read.isError === true ? <ErrorState compact error={read.error} onRetry={() => { void read.refetch?.(); }} /> : <SkeletonRows read={read} rows={3} />}
  </Dialog>;
}
