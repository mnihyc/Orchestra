/** What a dropdown's popup shows of its list, measured from the page: its box against its field's, and the rows inside it. */
import { expect, type Locator } from '@playwright/test';

export interface DropdownShown {
  popupTop: number; popupBottom: number; fieldTop: number; fieldBottom: number; screen: number;
  /** The options in the popup, and how many are wholly inside its scrolling part. */
  rows: number; whole: number;
  /** Whether the active option is wholly inside it. */
  active: boolean;
}

/** The popup of the combobox `field`, as it stands now. */
export async function dropdownShown(field: Locator): Promise<DropdownShown> {
  return field.evaluate((input) => {
    const popup = document.getElementById(input.getAttribute('aria-controls') ?? '');
    if (popup === null) throw new Error('The popup is not open');
    const popupBox = popup.getBoundingClientRect(), fieldBox = input.getBoundingClientRect();
    // With a footer the list scrolls inside the popup; without one the popup scrolls.
    const list = popup.querySelector<HTMLElement>('[data-combobox-footer]') === null ? popup : popup.querySelector<HTMLElement>('[data-combobox-options]') ?? popup;
    const frame = list.getBoundingClientRect();
    const rows = Array.from(popup.querySelectorAll<HTMLElement>('[role="option"]'));
    const inside = (element: Element) => { const box = element.getBoundingClientRect(); return box.top >= frame.top - 1 && box.bottom <= frame.bottom + 1; };
    return {
      popupTop: popupBox.top, popupBottom: popupBox.bottom, fieldTop: fieldBox.top, fieldBottom: fieldBox.bottom, screen: innerHeight,
      rows: rows.length, whole: rows.filter(inside).length, active: rows.filter((row) => row.getAttribute('aria-selected') === 'true').every(inside),
    };
  });
}

/** The room a popup promises (four rows, or its whole list), and the one thing it may never do: leave the screen, cover its field or cut its active row. */
export async function expectRoomKept(field: Locator, label: string, promised = 4): Promise<DropdownShown> {
  const shown = await dropdownShown(field);
  expect(shown.popupTop, `${label}: the popup stays on the screen`).toBeGreaterThanOrEqual(-1);
  expect(shown.popupBottom, `${label}: the popup stays on the screen`).toBeLessThanOrEqual(shown.screen + 1);
  expect(shown.popupBottom <= shown.fieldTop + 1 || shown.popupTop >= shown.fieldBottom - 1, `${label}: the popup does not cover its field`).toBe(true);
  expect(shown.whole, `${label}: the rows it promises are whole`).toBeGreaterThanOrEqual(Math.min(promised, shown.rows));
  expect(shown.active, `${label}: the active row is whole`).toBe(true);
  return shown;
}
