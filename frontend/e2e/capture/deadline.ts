/** Some Playwright response/body/storage promises have no per-call timeout of their own. */
import { READY_MS } from './config.ts';

export async function bounded<T>(operation: Promise<T>, timeout = READY_MS): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([operation, new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error('A capture operation exceeded its deadline.')), timeout);
    })]);
  } finally { if (timer !== undefined) clearTimeout(timer); }
}
