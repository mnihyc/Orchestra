/** Keep a tab's real name while distinguishing different view instances with that name. */
export interface TabCaption { id: string; title: string; kind?: string; path?: string }

export function tabCaptions(entries: readonly TabCaption[], instance: (name: string, number: number) => string): Map<string, string> {
  const captions = new Map(entries.map((entry) => [entry.id, entry.title]));
  const key = (value: string) => value.replace(/\s+/g, ' ').trim();
  const groups = () => {
    const held = new Map<string, TabCaption[]>();
    for (const entry of entries) { const name = key(captions.get(entry.id) ?? entry.title); held.set(name, [...held.get(name) ?? [], entry]); }
    return [...held.values()].filter((group) => group.length > 1);
  };
  for (const group of groups()) if (new Set(group.map((entry) => entry.kind)).size > 1) {
    for (const entry of group) if (entry.kind !== undefined && entry.kind !== entry.title) captions.set(entry.id, entry.title + ' · ' + entry.kind);
  }
  for (const group of groups()) if (new Set(group.map((entry) => entry.path)).size > 1) {
    for (const entry of group) if (entry.path !== undefined && entry.path !== entry.title) captions.set(entry.id, (captions.get(entry.id) ?? entry.title) + ' · ' + entry.path);
  }
  const reserved = new Set([...captions.values()].map(key));
  for (const group of groups()) {
    let number = 0;
    for (const entry of group) {
      const name = captions.get(entry.id) ?? entry.title;
      const formatted = new Set<string>();
      let shown: string;
      do {
        number += 1;
        const translated = instance(name, number);
        // Keep real names even before translations initialize; a broken formatter cannot loop.
        shown = formatted.has(translated) || !translated.includes(name) ? name + ' · ' + String(number) : translated;
        formatted.add(translated);
      } while (reserved.has(key(shown)));
      captions.set(entry.id, shown); reserved.add(key(shown));
    }
  }
  return captions;
}
