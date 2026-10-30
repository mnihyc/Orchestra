/** A place named so that no two places a reader can see read alike: its name alone, and where it is when that name is shared ("dep2 · Acme" against "dep2 · Beta").
 *
 * Display names may repeat: two orgs may each hold a department `dep2` with a project `web`, and their
 * addresses still name one each. A screen that shows a place's name to somebody who sees both says where each
 * is, and only then: a name nobody else among the places the reader sees carries is shown alone. Where it is,
 * is said in as few words as tell the places of that name apart, nearest first, in this order:
 *
 * 1. the root org ("web · Acme" against "web · Beta");
 * 2. the place it lies in ("web · dep1" against "web · dep2", both in Acme);
 * 3. the place it lies in and the root org ("web · dep2 · Acme" where Acme holds a `web` elsewhere too);
 * 4. the whole way up, nearest first.
 *
 * A root org is told apart from another of its name by its address. Names are compared as a reader compares
 * them: trimmed, in one case and one Unicode form. Two places of one name in one parent read alike to the end,
 * as they do in the tree, where they stand side by side. The text is the same in every language: names and a
 * middle dot, as the finder writes what lies under a name.
 */

/** A place as its label needs it. */
export interface Placed {
  id: string;
  name: string;
  /** The places above it, outermost first: its root org first, the place it lies in last; empty for a root org. A place the reader may not see has no name. */
  above: readonly { id: string; name: string | null }[];
  /** A root org's address, which tells two root orgs of one name apart. */
  address?: string | null | undefined;
}

/** What stands between a name and where it is. */
export const LABEL_SEPARATOR = ' · ';

/** A name as a reader compares it with another. */
export const foldName = (name: string): string => name.normalize('NFKC').trim().toLocaleLowerCase();

/** The places a reader can see, by their name as compared: what each label is told apart from. */
export type KnownPlaces = ReadonlyMap<string, readonly Placed[]>;

/** Groups places by their name as compared, each place once. */
export function knownPlaces(places: Iterable<Placed>): KnownPlaces {
  const groups = new Map<string, Map<string, Placed>>();
  for (const place of places) {
    const name = foldName(place.name);
    let group = groups.get(name);
    if (group === undefined) {
      group = new Map();
      groups.set(name, group);
    }
    group.set(place.id, place);
  }
  return new Map([...groups].map(([name, group]) => [name, [...group.values()]]));
}

/** The ways to say where a place is, shortest first; `null` where a way says nothing of this place. */
function contexts(place: Placed): (readonly string[] | null)[] {
  if (place.above.length === 0) return [place.address === null || place.address === undefined || place.address === '' ? null : [place.address]];
  const named = place.above.map((one) => one.name).filter((name): name is string => name !== null);
  const root = place.above[0]?.name ?? null;
  const parent = place.above.length > 1 ? (place.above.at(-1)?.name ?? null) : null;
  return [
    root === null ? null : [root],
    parent === null ? null : [parent],
    parent === null || root === null ? null : [parent, root],
    named.length === 0 ? null : named.reverse(),
  ];
}

const said = (words: readonly string[] | null | undefined): string | null => (words === null || words === undefined ? null : foldName(words.join(LABEL_SEPARATOR)));

/** The labels of places that share one name: each with the shortest way to say where it is that no other of them shares. */
function labelsOfGroup(group: readonly Placed[]): Map<string, string> {
  const ways = group.map(contexts);
  const labels = new Map<string, string>();
  group.forEach((place, index) => {
    const mine = ways[index] ?? [];
    const fallback = mine.at(-1) ?? null;
    let chosen: readonly string[] | null = null;
    for (const [form, words] of mine.entries()) {
      const text = said(words);
      if (text === null) continue;
      const shared = ways.some((theirs, other) => other !== index && said(theirs[form]) === text);
      if (!shared) {
        chosen = words;
        break;
      }
    }
    labels.set(place.id, [place.name, ...(chosen ?? fallback ?? [])].join(LABEL_SEPARATOR));
  });
  // Two places that took different ways may still read alike: those say the whole way up.
  const seen = new Map<string, string>();
  for (const place of group) {
    const label = foldName(labels.get(place.id) ?? place.name);
    const before = seen.get(label);
    if (before !== undefined) {
      for (const id of [before, place.id]) {
        const whole = contexts(group.find((one) => one.id === id) ?? place).at(-1) ?? null;
        const name = group.find((one) => one.id === id)?.name ?? place.name;
        labels.set(id, [name, ...(whole ?? [])].join(LABEL_SEPARATOR));
      }
    }
    seen.set(label, place.id);
  }
  return labels;
}

/**
 * The label of each place of a list: its name, or its name with where it is when another place the reader can see
 * (one of `known`, or another row of the same list) reads the same.
 */
export function placeLabels(places: readonly Placed[], known: KnownPlaces = new Map()): Map<string, string> {
  const listed = knownPlaces(places);
  const labels = new Map<string, string>();
  const done = new Map<string, Map<string, string>>();
  for (const place of places) {
    const name = foldName(place.name);
    let group = done.get(name);
    if (group === undefined) {
      const together = new Map<string, Placed>();
      for (const one of known.get(name) ?? []) together.set(one.id, one);
      // A row of the list knows best where it is: it is what the reader is shown.
      for (const one of listed.get(name) ?? []) together.set(one.id, one);
      group = together.size <= 1 ? new Map([[place.id, place.name]]) : labelsOfGroup([...together.values()]);
      done.set(name, group);
    }
    labels.set(place.id, group.get(place.id) ?? place.name);
  }
  return labels;
}

/** The label of one place among the places a reader can see. */
export function placeLabel(place: Placed, known: KnownPlaces = new Map()): string {
  return placeLabels([place], known).get(place.id) ?? place.name;
}

/** A place from the way to it, as the API gives one (`path`: the root org first, the place itself last). */
export function placedOfPath(path: readonly { id: string; name: string | null }[], address?: string | null): Placed | null {
  const self = path.at(-1);
  if (self?.name === undefined || self.name === null) return null;
  return { id: self.id, name: self.name, above: path.slice(0, -1), address };
}
