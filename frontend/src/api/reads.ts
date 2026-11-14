/** A superseded or cancelled read has no failure to present and must release its loading state. */
import { CancelledError, type QueryClient, type QueryKey } from '@tanstack/react-query';
import { NetworkError } from '@/lib/requests';

export function supersededRead(): CancelledError { return new CancelledError({ revert: true }); }

export function isReadCancelled(error: unknown): boolean {
  return error instanceof CancelledError || error instanceof NetworkError && error.failure === 'aborted'
    || error instanceof Error && error.name === 'AbortError';
}

/** Query cancellation must go through its owner: merely throwing a silent error leaves its fetch busy. */
export async function queryRead<T>(cache: QueryClient, key: QueryKey, read: () => T | Promise<T>): Promise<T> {
  const owner = cache.getQueryCache().find({ queryKey: key, exact: true });
  const flight = owner?.promise;
  try { return await read(); }
  catch (error) {
    if (!isReadCancelled(error)) throw error;
    if (owner?.promise === flight) await owner?.cancel({ revert: true });
    throw supersededRead();
  }
}
