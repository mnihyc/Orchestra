/** A controlled response for exercising an action while its request is still pending. */
export function deferred<T>() {
  let accept: ((value: T) => void) | undefined;
  let refuse: ((error: unknown) => void) | undefined;
  const promise = new Promise<T>((resolve, reject) => { accept = resolve; refuse = reject; });
  return {
    promise,
    resolve: (value: T) => {
      if (accept === undefined) throw new Error('The request has no resolver.');
      accept(value);
    },
    reject: (error: unknown) => {
      if (refuse === undefined) throw new Error('The request has no rejecter.');
      refuse(error);
    },
  };
}
