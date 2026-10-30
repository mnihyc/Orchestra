/** Whether a name stands alone or inside something that is pressed itself.
 *
 * A name opens its card, or a count its list, only where it is not part of a button, a link or a choice:
 * there what it stands in acts, and a second thing to press inside the first would be neither. */

/** What is pressed itself: a name inside one of these is part of what it says. */
export const PRESSED = [
  'a[href]',
  'button',
  'label',
  'summary',
  '[role="button"]',
  '[role="link"]',
  '[role="option"]',
  '[role="menuitem"]',
  '[role="menuitemradio"]',
  '[role="menuitemcheckbox"]',
  '[role="radio"]',
  '[role="checkbox"]',
  '[role="switch"]',
  '[role="tab"]',
  '[role="treeitem"]',
].join(',');

/** Whether the element is drawn outside everything that is pressed; one not drawn yet is not. */
export function standsAlone(element: Element | null): boolean {
  const parent = element?.parentElement ?? null;
  return parent !== null && parent.closest(PRESSED) === null;
}
