/** A failed first-use part stays failed until the boundary showing that error is explicitly retried. */
const retries = new WeakMap<Error, Set<() => void>>();

export function partFailure(error: unknown, retry: () => void): Error {
  const failure = error instanceof Error ? error : new Error('This part did not arrive.', { cause: error });
  const attempts = retries.get(failure) ?? new Set<() => void>();
  attempts.add(retry); retries.set(failure, attempts);
  return failure;
}

export function retryPart(error: unknown): void {
  if (!(error instanceof Error)) return;
  const attempts = retries.get(error);
  retries.delete(error);
  for (const retry of attempts ?? []) retry();
}
