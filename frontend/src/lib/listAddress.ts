/**
 * One state of a screen has one address. The parts of an address that carry a list's state became known one after another (the marker
 * when the page was entered, each control's choice when it came on screen, a remembered state, a link), and the address must not say
 * in which sequence: it is written, whoever writes it, with the parameters of the screen first, then `list` (the choices, by their
 * names), then the marker `_list`. Copied links, the browser's history and a remembered last place then see one address.
 */
import { defaultStringifySearch } from '@tanstack/react-router';

type Search = Record<string, unknown>;

export const objectOf = (value: unknown): Search | undefined => typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Search : undefined;

/** The search of a page in its one order: its own parameters as they stand, the choices by name, then the marker. */
export function canonicalListSearch(search: Search): Search {
  const { list, _list, ...own } = search;
  const choices = objectOf(list);
  return {
    ...own,
    ...(list === undefined ? {} : { list: choices === undefined ? list : Object.fromEntries(Object.entries(choices).sort(([one], [other]) => (one < other ? -1 : one > other ? 1 : 0))) }),
    ...(_list === undefined ? {} : { _list }),
  };
}

/** What the router writes for a search object, and what every address of a page is built with. */
export function canonicalStringifySearch(search: Search): string {
  return defaultStringifySearch(canonicalListSearch(search));
}
