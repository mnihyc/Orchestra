/** Quoted code cannot end its own Markdown fence, even in large streamed output. */
export function fencedCode(body: string, language = ''): string {
  let longest = 2;
  for (const run of body.matchAll(/`+/g)) longest = Math.max(longest, run[0].length);
  const fence = '`'.repeat(longest + 1);
  return `${fence}${language}\n${body}\n${fence}`;
}
