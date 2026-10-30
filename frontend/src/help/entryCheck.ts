/** Screen audits compare visible guide destinations and their accessible names in the reader's language. */
export interface GuideEntry { article: string; heading: string | null; name: string }

export function checkGuideEntries(entries: readonly GuideEntry[]): string[] {
  const failures: string[] = [];
  const destinations = new Set<string>();
  const names = new Set<string>();
  for (const entry of entries) {
    const destination = `${entry.article}#${entry.heading ?? ''}`;
    const name = entry.name.normalize('NFKC').replace(/\s+/g, ' ').trim().toLowerCase();
    if (destinations.has(destination)) failures.push(`help.destination_repeated: ${destination}`);
    if (name === '') failures.push(`help.name_missing: ${destination}`);
    else if (names.has(name)) failures.push(`help.name_repeated: ${name}`);
    destinations.add(destination); names.add(name);
  }
  return [...new Set(failures)];
}
