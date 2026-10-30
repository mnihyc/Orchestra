/** Class names joined: the parts that are strings, in order, with everything else left out. */
export function cx(...parts: (string | false | null | undefined)[]): string {
  let joined = '';
  for (const part of parts) {
    if (typeof part !== 'string' || part === '') continue;
    joined = joined === '' ? part : `${joined} ${part}`;
  }
  return joined;
}
