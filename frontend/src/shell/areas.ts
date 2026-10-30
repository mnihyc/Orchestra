/** Visible keyboard areas, shared by the skip link and F6. Detached or covered views are not stops. */
export function mainContent(): HTMLElement | null {
  const main = document.querySelector<HTMLElement>('[data-main-area]');
  if (main !== null && visible(main)) return main;
  return Array.from(document.querySelectorAll<HTMLElement>('[data-revealed-view],[data-workbench-bar]')).find(visible) ?? null;
}

function visible(element: HTMLElement): boolean {
  return element.isConnected && element.closest('[hidden],[inert],[aria-hidden="true"]') === null
    && element.getClientRects().length > 0 && getComputedStyle(element).visibility !== 'hidden';
}

export function areaStops(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-area-stop],[data-view-group]')).filter((element) =>
    visible(element) && !(element.hasAttribute('data-main-area') && element.querySelector('[data-view-group]') !== null));
}

export function focusArea(step: 1 | -1): boolean {
  const stops = areaStops();
  if (stops.length === 0) return false;
  const active = document.activeElement;
  const found = stops.findIndex((stop) => stop.contains(active) || (active instanceof HTMLElement && active.hasAttribute('data-main-area') && active.contains(stop)));
  const from = found >= 0 ? found : step === 1 ? -1 : 0;
  const next = stops[(from + step + stops.length) % stops.length];
  if (next === undefined) return false;
  const controls = Array.from(next.querySelectorAll<HTMLElement>('[role="tab"][tabindex="0"],a[href],button:not([disabled]),input:not([disabled]),[tabindex="0"]')).filter(visible);
  const target = controls.find((control) => control.getAttribute('tabindex') === '0') ?? controls[0] ?? next;
  target.focus();
  return document.activeElement === target;
}
