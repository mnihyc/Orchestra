/** Measurements run inside the browser, against rendered styles and text rather than class names. */
export interface DuplicateAllowance {
  text: string;
  /** Every permitted occurrence has a specific selector; no page-wide exemption. */
  selectors: string[];
  reason: string;
}
export interface LayoutInput { duplicateAllowList: DuplicateAllowance[]; rawValues: string[]; fullValueDisclosure?: { reason: string }; mode?: 'choices' | 'icons'; settle?: boolean; pointerTarget?: string; afterInput?: { kind: 'key' | 'pointer'; sequence: number } }
export interface LayoutFault { rule: string; text: string; selectors: string[]; detail: string }
export interface CutValue { selector: string; focus: string; text: string; title: string | null }
export interface LayoutResult { faults: LayoutFault[]; cuts: CutValue[]; allowances: string[]; icons: string[]; menus: string[] }

/** Record the real trusted input path; a CSS pseudo-class alone need not identify its hit target. */
export function trackLayoutPointer(): void {
  const scope = globalThis as typeof globalThis & { __layoutPointer?: { root: Element; targets: EventTarget[]; keySerial: number; pointerSerial: number } };
  if (scope.__layoutPointer?.root === document.documentElement) return;
  const pointer = { root: document.documentElement, targets: [] as EventTarget[], keySerial: 0, pointerSerial: 0 };
  scope.__layoutPointer = pointer;
  document.addEventListener('keyup', (event) => { if (event.isTrusted) pointer.keySerial += 1; }, true);
  document.addEventListener('pointermove', (event) => { if (event.isTrusted) { pointer.targets = event.composedPath(); pointer.pointerSerial += 1; } }, true);
}

export interface LayoutPointer { selector: string; x: number; y: number; tooltips: number }
/** Prepare the next real pointer target after the current measurement, within the same browser turn. */
export function prepareLayoutPointer(selectors: string[]): LayoutPointer | null {
  for (const selector of selectors) {
    const target = document.querySelector(selector);
    if (!(target instanceof HTMLElement) || getComputedStyle(target).visibility === 'hidden') continue;
    const before = target.getBoundingClientRect();
    if (before.width === 0 || before.height === 0) continue;
    target.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' });
    const box = target.getBoundingClientRect();
    return { selector, x: box.left + box.width / 2, y: box.top + box.height / 2, tooltips: document.querySelectorAll('[role="tooltip"]').length };
  }
  return null;
}

/** Self-contained because Playwright serializes this function into the page. */
export async function inspectLayout(input: LayoutInput): Promise<LayoutResult> {
  // Wait on the actual trusted event, so its driver acknowledgement can finish while the two
  // render frames settle. No measurement can run ahead of the input it is checking.
  if (input.afterInput !== undefined) {
    const after = input.afterInput;
    const ready = () => {
      const state = (globalThis as typeof globalThis & { __layoutPointer?: { keySerial: number; pointerSerial: number } }).__layoutPointer;
      return state !== undefined && (after.kind === 'key' ? state.keySerial : state.pointerSerial) > after.sequence;
    };
    if (!ready()) await new Promise<void>((done) => {
      const event = after.kind === 'key' ? 'keyup' : 'pointermove';
      const checked = () => { if (ready()) { document.removeEventListener(event, checked, true); done(); } };
      document.addEventListener(event, checked, true); checked();
    });
  }
  if (input.settle === true) await new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done())));
  if (input.pointerTarget !== undefined) {
    const target = document.querySelector(input.pointerTarget);
    const path = (globalThis as typeof globalThis & { __layoutPointer?: { targets: EventTarget[] } }).__layoutPointer?.targets;
    if (target === null || path?.includes(target) !== true) throw new Error(`The pointer did not reach ${input.pointerTarget}`);
  }
  const faults: LayoutFault[] = [];
  const cuts: CutValue[] = [];
  const allowances: string[] = [];
  const normalize = (text: string) => text.replace(/\s+/g, ' ').trim();
  // A measurement has no intervening input or render. Reuse each rectangle/style only within
  // this call; the next keyboard/pointer state measures everything afresh.
  const boxes = new Map<Element, DOMRect>();
  const boxOf = (element: Element) => {
    let box = boxes.get(element);
    if (box === undefined) { box = element.getBoundingClientRect(); boxes.set(element, box); }
    return box;
  };
  // CSSOM computed declarations are browser-live objects, not snapshots. Reuse only
  // the handles; every property below is read again after the actual input and frames.
  // Geometry, visibility, text and all derived values remain local to this measurement.
  const live = globalThis as typeof globalThis & { __layoutStyleHandles?: { root: Element; values: WeakMap<Element, CSSStyleDeclaration> } };
  if (live.__layoutStyleHandles?.root !== document.documentElement) live.__layoutStyleHandles = { root: document.documentElement, values: new WeakMap() };
  const styles = live.__layoutStyleHandles.values;
  const styleOf = (element: Element) => {
    let style = styles.get(element);
    if (style === undefined) { style = getComputedStyle(element); styles.set(element, style); }
    return style;
  };
  const summaries = new Map<HTMLDetailsElement, Element | null>();
  // A closed disclosure paints its first summary only. Other contents can still
  // return nonzero client boxes, so resolve this native state before measuring them.
  function closedDisclosureContent(element: Element): boolean {
    const parent = element.parentElement;
    if (!(parent instanceof HTMLDetailsElement) || parent.open) return false;
    let summary = summaries.get(parent);
    if (summary === undefined) { summary = parent.querySelector(':scope > summary'); summaries.set(parent, summary); }
    return element !== summary;
  }
  for (const element of Array.from(document.querySelectorAll('[data-layout-probe]'))) element.removeAttribute('data-layout-probe');
  let sequence = 0;
  function mark(element: Element): string {
    let id = element.getAttribute('data-layout-probe');
    if (id === null) { sequence += 1; id = String(sequence); element.setAttribute('data-layout-probe', id); }
    return `[data-layout-probe="${id}"]`;
  }
  const visibleAtRest = new Map<Element, boolean>(), visibleOnHover = new Map<Element, boolean>();
  interface VisibilityFrame { blocked: boolean; transparent: boolean; left: number; right: number; top: number; bottom: number }
  const unbounded: VisibilityFrame = { blocked: false, transparent: false, left: -Infinity, right: Infinity, top: -Infinity, bottom: Infinity };
  const visibilityFrames = new Map<Element, VisibilityFrame>();
  // The same ancestor chain constrains many text nodes. Summarize it once in this measurement,
  // keeping each clipping-axis comparison independent, just as the direct ancestor walk does.
  // No rectangle, style or visibility summary survives the next input state.
  function visibilityFrame(element: Element): VisibilityFrame {
    const saved = visibilityFrames.get(element);
    if (saved !== undefined) return saved;
    const parent = element.parentElement === null ? unbounded : visibilityFrame(element.parentElement);
    if (parent.blocked || closedDisclosureContent(element)) {
      const frame = { ...parent, blocked: true };
      visibilityFrames.set(element, frame); return frame;
    }
    const style = styleOf(element), box = boxOf(element), legacyClip = style.getPropertyValue('clip');
    const clippedLabel = box.width <= 1 && box.height <= 1 && ((legacyClip !== 'auto' && legacyClip !== '') || (style.clipPath !== 'none' && style.clipPath !== ''));
    const clipsX = ['hidden', 'clip', 'auto', 'scroll'].includes(style.overflowX);
    const clipsY = ['hidden', 'clip', 'auto', 'scroll'].includes(style.overflowY);
    const frame: VisibilityFrame = {
      blocked: element.matches('script,style,template,[hidden],[inert]') || style.display === 'none' || style.visibility === 'hidden' || clippedLabel,
      transparent: parent.transparent || Number(style.opacity || '1') === 0,
      left: clipsX ? Math.max(parent.left, box.left) : parent.left,
      right: clipsX ? Math.min(parent.right, box.right) : parent.right,
      top: clipsY ? Math.max(parent.top, box.top) : parent.top,
      bottom: clipsY ? Math.min(parent.bottom, box.bottom) : parent.bottom,
    };
    visibilityFrames.set(element, frame); return frame;
  }
  function visible(element: Element, includeHoverOnly = false): boolean {
    const cache = includeHoverOnly ? visibleOnHover : visibleAtRest;
    const saved = cache.get(element);
    if (saved !== undefined) return saved;
    const result = measureVisibility(element, includeHoverOnly);
    cache.set(element, result); return result;
  }
  function measureVisibility(element: Element, includeHoverOnly: boolean): boolean {
    const frame = visibilityFrame(element);
    if (frame.blocked || (!includeHoverOnly && frame.transparent)) return false;
    // A nonzero bounding union already proves there are client rects. Reuse that
    // geometry instead of asking layout for the same element a second time.
    const own = boxOf(element);
    if (own.width === 0 || own.height === 0) return false;
    const parent = element.parentElement === null ? unbounded : visibilityFrame(element.parentElement);
    return !(own.right <= parent.left || own.left >= parent.right || own.bottom <= parent.top || own.top >= parent.bottom);
  }
  function fail(rule: string, text: string, elements: Element[], detail: string) {
    faults.push({ rule, text, selectors: elements.map(mark), detail });
  }
  const directTexts = new Map<Element, string>();
  function directText(element: Element): string {
    const saved = directTexts.get(element);
    if (saved !== undefined) return saved;
    if (element instanceof HTMLDetailsElement && !element.open) { directTexts.set(element, ''); return ''; }
    const parts: string[] = [];
    for (let node = element.firstChild; node !== null; node = node.nextSibling) if (node.nodeType === Node.TEXT_NODE) parts.push(node.textContent ?? '');
    const text = normalize(parts.join(' '));
    directTexts.set(element, text); return text;
  }
  // These values are stable only during this synchronous measurement. The next actual
  // key/pointer state creates fresh maps, while repeated rules share each native read.
  const renderedTexts = new Map<HTMLElement, string>();
  function renderedText(element: HTMLElement): string {
    let text = renderedTexts.get(element);
    if (text === undefined) { text = element.innerText; renderedTexts.set(element, text); }
    return text;
  }
  const originals = new Map<HTMLElement, string | null>();
  function originalValue(element: HTMLElement): string | null {
    const saved = originals.get(element);
    if (saved !== undefined) return saved;
    const recorded = element.getAttribute('data-overflow-text');
    if (recorded !== null) { originals.set(element, recorded); return recorded; }
    const shown = normalize(renderedText(element)), title = element.getAttribute('title');
    const result = shown.includes('…') && title !== null && title.length > shown.length ? title : null;
    originals.set(element, result); return result;
  }
  const textLoss = new Map<HTMLElement, boolean>();
  function losesText(element: HTMLElement): boolean {
    const saved = textLoss.get(element);
    if (saved !== undefined) return saved;
    const style = styleOf(element), original = originalValue(element);
    const clamp = Number.parseInt(style.getPropertyValue('-webkit-line-clamp') || style.getPropertyValue('line-clamp'), 10) > 0;
    const result = ((style.textOverflow.includes('ellipsis') || clamp) && (element.scrollWidth > element.clientWidth || element.scrollHeight > element.clientHeight)) ||
      (original !== null && normalize(original) !== normalize(renderedText(element)));
    textLoss.set(element, result); return result;
  }
  // Both transient choice rules operate only on direct text. Empty wrappers never enter either
  // rule; avoid measuring their visibility again for every keyboard/pointer highlight.
  // Hidden retained views cannot contribute visible descendants. Reject only the same
  // inherited hidden/inert/display/visibility/zero-opacity states as visibilityFrame;
  // zero-size parents and clipping stay individual checks because descendants can escape.
  const hiddenSubtree = (element: Element): boolean => {
    if (closedDisclosureContent(element)) return true;
    const style = styleOf(element);
    return element.matches('script,style,template,[hidden],[inert]') || style.display === 'none' || style.visibility === 'hidden' || Number(style.opacity || '1') === 0;
  };
  const elements: HTMLElement[] = [];
  if (input.mode === 'icons') {
    for (const element of Array.from(document.querySelectorAll('button,[role="button"]'))) if (element instanceof HTMLElement && visible(element)) elements.push(element);
  } else if (!hiddenSubtree(document.body)) {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT, {
      acceptNode(node) {
        const element = node as Element;
        if (hiddenSubtree(element)) return NodeFilter.FILTER_REJECT;
        if (!(element instanceof HTMLElement) || input.mode === 'choices' && directText(element) === '' || !visible(element)) return NodeFilter.FILTER_SKIP;
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) elements.push(node as HTMLElement);
  }
  const controls = elements.filter((element) => element.matches('button,a[href],input,select,textarea,[role="button"],[role="tab"],[role="radio"],[role="checkbox"],[role="switch"],[role="combobox"]'));
  const nameOf = (element: HTMLElement) => {
    const labelled = element.getAttribute('aria-labelledby')?.split(/\s+/).map((id) => document.getElementById(id)?.textContent ?? '').join(' ');
    // Empty labels fall through as well as missing ones; ?? alone would lose the visible name.
    const name = [labelled, element.getAttribute('aria-label'), renderedText(element), element.getAttribute('title')]
      .find((value) => value !== undefined && value !== null && value.trim() !== '');
    return normalize(name ?? '');
  };
  const repeatedRow = 'li,tr,[role="row"],[role="treeitem"],[role="listitem"],[role="option"],[role="menuitem"],[role="menuitemradio"],[role="menuitemcheckbox"],[data-item],[data-timeline-row]';
  // A transient layer is read on its own, apart from the page or layer beneath it.
  // Identify actual native layers and shared floating content, never particular words.
  // Duplicates within one layer retain every ordinary action, row and content check.
  const layerRoots = '[role="dialog"],[role="alertdialog"],[role="menu"],dialog[open],.floating-layer';
  const layerScopes = new Map<Element, number>();
  function layerOf(element: Element): number {
    const root = element.closest(layerRoots);
    if (root === null) return 0;
    const scope = layerScopes.get(root) ?? layerScopes.size + 1;
    layerScopes.set(root, scope);
    return scope;
  }
  function inRepeatedRow(element: Element): boolean {
    const row = element.closest(repeatedRow);
    return row !== null && layerOf(row) === layerOf(element);
  }
  const technical = 'pre,code,kbd,input,textarea,[contenteditable="true"],[data-user-content],.cm-editor,.xterm';
  // One CSS pixel permits fractional rectangle rounding, as in the other geometry rules.
  const edgeRounding = 1;
  const layers = Array.from(document.querySelectorAll('[data-radix-popper-content-wrapper] > *, .floating-layer, [data-overflow-tooltip]'))
    .filter((element): element is HTMLElement => element instanceof HTMLElement && visible(element));
  for (const layer of layers) {
    const rect = boxOf(layer);
    if (rect.left < -edgeRounding || rect.top < -edgeRounding || rect.right > innerWidth + edgeRounding || rect.bottom > innerHeight + edgeRounding) {
      fail('floating-bounds', nameOf(layer), [layer], 'The floating layer must stay inside the viewport.');
    }
    if (layer.scrollHeight > layer.clientHeight + edgeRounding) {
      const scrolls = (element: HTMLElement) => ['auto', 'scroll'].includes(styleOf(element).overflowY)
        && element.clientHeight > 0 && element.scrollHeight > element.clientHeight + edgeRounding;
      if (!scrolls(layer) && !Array.from(layer.querySelectorAll<HTMLElement>('*')).some((child) => visible(child) && scrolls(child))) {
        fail('floating-scroll', nameOf(layer), [layer], 'Long floating content must scroll inside its layer.');
      }
    }
  }
  // Transient probes repeat their complete rule over the page, without rerunning unrelated rules.
  // The ordinary page/menu pass still measures every rule and all full-value disclosures.
  if (input.mode === 'icons') {
    checkIcons();
    return { faults, cuts, allowances, icons: [], menus: [] };
  }

  if (input.mode === undefined) {
  // Text alone cannot see an icon-only Star; the AX rule below is deliberately landmark-local.
  // Compare separate actions across landmarks within their own layer, including pressed/icon buttons.
  const actions = new Map<string, HTMLElement[]>();
  for (const control of controls) {
    if (!control.matches('button,a[href],[role="button"]') || control.matches('[role="tab"],[role="radio"],[role="checkbox"],[role="switch"],[role="combobox"]') || inRepeatedRow(control)) continue;
    const name = nameOf(control).normalize('NFKC').toLocaleLowerCase();
    if (name === '') continue;
    const key = `${layerOf(control)}:${name}`;
    const prior = actions.get(key) ?? [];
    prior.push(control); actions.set(key, prior);
  }
  for (const matches of actions.values()) {
    const first = matches[0];
    if (first === undefined || matches.length < 2) continue;
    fail('duplicate-action', nameOf(first), matches, 'The same named action appears twice within one layer, including icon-only controls and across landmarks. Qualify different subjects; keep repeated object-row actions in their rows.');
  }

  // Row actions remain menus; toolbar/tab overflow and inert hidden-item ellipses do not.
  for (const control of controls) {
    const name = nameOf(control);
    const toolbarOverflow = control.closest('[role="toolbar"],[role="tablist"],[data-workbench-bar]') !== null && /^(?:More\b|更多)/iu.test(name);
    if (control.hasAttribute('data-overflow-trigger') || toolbarOverflow || /^(?:More(?: tabs| sections| tools| views| options)?|All tabs(?: in .+)?|更多(?:标签|选项|分区)?|所有(?:标签|选项卡)|.+中的所有标签)[….]*$/iu.test(name)) {
      fail('more-overflow', name, [control], 'Wrap or compact these controls instead of putting them behind an overflow.');
    }
  }
  for (const element of elements) {
    if (/^(?:…|\.{3})$/.test(directText(element)) && element.closest('button,a[href],summary,[role="button"],[aria-expanded],[aria-haspopup],[data-user-content],pre,code,[role="status"]') === null) {
      fail('inert-ellipsis', directText(element), [element], 'An ellipsis for hidden items must be a keyboard-operable disclosure.');
    }
  }

  // Count the interface's own words, once at their direct owner. Verbatim source/file
  // bodies are data; authored document words compare within their real block. Controls
  // and noneditable interface toolbars retain the ordinary check, even inside an editor.
  // Content roles identify these regions; no string or filename can grant an exemption.
  const contentRoles = '[data-content-role="source"],[data-content-role="document"]';
  const interfaceWords = 'button,a[href],input,select,textarea,summary,[role="button"],[role="tab"],[role="radio"],[role="checkbox"],[role="switch"],[role="combobox"]';
  const contentScopes = new Map<Element, number>();
  const occurrences = new Map<string, { text: string; matches: HTMLElement[] }>();
  function ownedTabHeading(matches: HTMLElement[]): boolean {
    if (matches.length !== 2) return false;
    const labels = matches.map((element) => element.closest<HTMLElement>('[role="tab"]'));
    const index = labels.findIndex((tab) => tab !== null);
    if (index < 0 || labels.filter((tab) => tab !== null).length !== 1) return false;
    const tab = labels[index]!;
    const heading = matches[1 - index]!.closest<HTMLElement>('h1,h2,h3,h4,h5,h6,[role="heading"]');
    const controlled = (tab.getAttribute('aria-controls') ?? '').trim().split(/\s+/);
    if (tab.id === '' || tab.getAttribute('aria-selected') !== 'true' || controlled.length !== 1) return false;
    const panel = document.getElementById(controlled[0] ?? '');
    if (panel?.getAttribute('role') !== 'tabpanel' || heading?.closest('[role="tabpanel"]') !== panel
      || !(panel.getAttribute('aria-labelledby') ?? '').split(/\s+/).includes(tab.id)) return false;
    const headings = Array.from(panel.querySelectorAll<HTMLElement>('h1,h2,h3,h4,h5,h6,[role="heading"]'))
      .filter((element) => element.closest('[role="tabpanel"]') === panel);
    const primary = headings.find((element) => element.matches('h1,[role="heading"][aria-level="1"]'))
      ?? headings.find((element) => visible(element));
    return heading === primary && normalize(renderedText(tab)) === normalize(renderedText(heading));
  }
  for (const element of elements) {
    if (inRepeatedRow(element)) continue;
    const content = element.closest(contentRoles);
    const control = element.closest(interfaceWords);
    const editableBoundary = element.closest('[contenteditable]');
    const interfaceText = control !== null && control !== content || editableBoundary !== null && editableBoundary !== content && editableBoundary.getAttribute('contenteditable') === 'false';
    if (content?.getAttribute('data-content-role') === 'source' && !interfaceText) continue;
    const block = content?.getAttribute('data-content-role') === 'document' && !interfaceText
      ? element.closest('[data-ob="block"],[data-ob="leaf"]') ?? content : null;
    let scope = 0;
    if (block !== null) { scope = contentScopes.get(block) ?? contentScopes.size + 1; contentScopes.set(block, scope); }
    const text = directText(element);
    if (Array.from(text).length < 3) continue;
    const key = `${layerOf(element)}:${scope}:${text}`;
    const prior = occurrences.get(key) ?? { text, matches: [] };
    prior.matches.push(element); occurrences.set(key, prior);
  }
  for (const { text, matches } of occurrences.values()) {
    if (matches.length < 2) continue;
    // A tab navigates and its own panel heading heads the page: both keep their name.
    // Exactly this selected-tab/primary-heading pair may repeat, proven by mutual native
    // ARIA ownership. A third copy, another panel/heading or duplicate controls still fail.
    if (ownedTabHeading(matches)) { allowances.push(`${text}: selected tab and its own panel heading`); continue; }
    const allowed = input.duplicateAllowList.find((entry) => entry.text === text && entry.reason.trim() !== '' &&
      entry.selectors.length === matches.length && entry.selectors.every((selector) => selector !== '*' && matches.filter((element) => element.matches(selector)).length === 1) &&
      matches.every((element) => entry.selectors.some((selector) => element.matches(selector))));
    if (allowed !== undefined) { allowances.push(`${text}: ${allowed.reason}`); continue; }
    // A full-value disclosure may also repeat its own actually cut value. Validate the
    // one original, actual clipping and ARIA ownership; no arbitrary repeated tooltip text is excused.
    const originals = matches.filter((element) => element.closest('[role="tooltip"],[data-tooltip-content]') === null);
    const disclosed = matches.filter((element) => element.closest('[role="tooltip"],[data-tooltip-content]') !== null);
    const original = originals[0];
    const ownedDisclosure = input.fullValueDisclosure !== undefined && input.fullValueDisclosure.reason.trim() !== '' && originals.length === 1 && original !== undefined && disclosed.length > 0 && disclosed.every((element) => {
      const root = element.closest('[data-tooltip-content],[role="tooltip"]');
      const tooltip = root?.getAttribute('role') === 'tooltip' ? root : root?.querySelector('[role="tooltip"]');
      if (tooltip === null || tooltip === undefined || tooltip.id === '') return false;
      let cut = false, owned = false;
      for (let source: HTMLElement | null = original; source !== null && source !== document.body; source = source.parentElement) {
        cut ||= losesText(source);
        owned ||= (source.getAttribute('aria-describedby') ?? '').split(/\s+/).includes(tooltip.id);
      }
      return cut && owned;
    });
    if (ownedDisclosure) allowances.push(`${text}: ${input.fullValueDisclosure?.reason ?? ''}`);
    else fail('duplicate-text', text, matches, 'Visible text of at least three characters occurs outside repeated rows more than once.');
  }

  for (const element of elements.filter((element) => element.matches('[data-choice-description],[data-choice-selected-description],[data-choice-descriptions]'))) {
    if (element.closest('[role="tooltip"],[data-tooltip-content],[role="menu"],[role="listbox"]') === null) {
      fail('choice-description', normalize(renderedText(element)), [element], 'Keep the in-page control compact; put guidance in its tooltip or an open menu.');
    }
  }
  // Source/code and editable user data deliberately retain machine-readable spelling. UI labels do not.
  const rawKey = /\b[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+\b/u;
  for (const element of elements) {
    if (element.closest(`${technical},[role="tooltip"],[data-tooltip-content]`) !== null) continue;
    const text = directText(element);
    const raw = input.rawValues.find((value) => text.split(/\s+/).some((part) => part.replace(/^[“"'(]+|[”"'),:;.!?]+$/g, '') === value));
    const key = rawKey.exec(text)?.[0];
    if (raw !== undefined || key !== undefined) fail('raw-name', text, [element], `Show the catalogue/display name of ${raw ?? key ?? ''}.`);
  }
  }

  // Resolve CSS Color 4 through the browser's sRGB canvas, then composite translucent boundaries.
  type Colour = [number, number, number, number];
  const canvas = document.createElement('canvas'); canvas.width = 1; canvas.height = 1;
  const context = canvas.getContext('2d', { willReadFrequently: true });
  if (context === null) throw new Error('The browser cannot measure text and control colours.');
  const paint = context;
  // Canvas word widths depend on the font, language and word, not on a hover background or box.
  // Keep those exact measurements within this document; a font finishing invalidates them.
  type TextWidths = Map<string, number>;
  type FontWords = Map<string, Map<string, TextWidths>>;
  type SpacingWidths = Map<number, TextWidths>;
  type TransformWidths = Map<string, SpacingWidths>;
  type FontStyles = Map<string, TransformWidths>;
  type StyledWords = Map<string, FontStyles>;
  interface WordWidths { fonts: FontFaceSet; values: FontWords; longest: StyledWords; fontNames: Map<string, string> }
  const scope = globalThis as typeof globalThis & { __layoutWordWidths?: WordWidths; __layoutColours?: Map<string, Colour> };
  let wordWidths = scope.__layoutWordWidths;
  if (wordWidths?.fonts !== document.fonts) {
    const fresh: WordWidths = { fonts: document.fonts, values: new Map(), longest: new Map(), fontNames: new Map() };
    document.fonts.addEventListener('loadingdone', () => { fresh.values.clear(); fresh.longest.clear(); });
    scope.__layoutWordWidths = fresh;
    wordWidths = fresh;
  }
  function bucket<Key, Value>(map: Map<Key, Value>, key: Key, create: () => Value): Value {
    let value = map.get(key);
    if (value === undefined) { value = create(); map.set(key, value); }
    return value;
  }
  const measuredWords = wordWidths.values;
  // Parsing a resolved CSS colour is pure; every rendered foreground/ground is still read now.
  const colours = scope.__layoutColours ??= new Map<string, Colour>();
  function colour(value: string): Colour {
    const held = colours.get(value);
    if (held !== undefined) return held;
    paint.clearRect(0, 0, 1, 1); paint.fillStyle = value; paint.fillRect(0, 0, 1, 1);
    const pixel = paint.getImageData(0, 0, 1, 1).data;
    const result: Colour = [pixel[0] ?? 0, pixel[1] ?? 0, pixel[2] ?? 0, (pixel[3] ?? 0) / 255];
    colours.set(value, result); return result;
  }
  function over(front: Colour, back: Colour): Colour {
    const alpha = front[3] + back[3] * (1 - front[3]);
    if (alpha === 0) return [0, 0, 0, 0];
    const channel = (index: 0 | 1 | 2) => (front[index] * front[3] + back[index] * back[3] * (1 - front[3])) / alpha;
    return [channel(0), channel(1), channel(2), alpha];
  }
  const backgroundCache = new Map<Element, Colour>();
  function background(element: Element | null): Colour {
    if (element === null) return [255, 255, 255, 1];
    const saved = backgroundCache.get(element);
    if (saved !== undefined) return saved;
    const result = over(colour(styleOf(element).backgroundColor), background(element.parentElement));
    backgroundCache.set(element, result); return result;
  }
  function light(value: Colour): number {
    const channels = value.slice(0, 3).map((part) => part / 255).map((part) => part <= 0.04045 ? part / 12.92 : ((part + 0.055) / 1.055) ** 2.4);
    return (channels[0] ?? 0) * 0.2126 + (channels[1] ?? 0) * 0.7152 + (channels[2] ?? 0) * 0.0722;
  }
  function contrast(first: Colour, second: Colour): number {
    const one = light(first), two = light(second);
    return (Math.max(one, two) + 0.05) / (Math.min(one, two) + 0.05);
  }

  // Include ordinary labels and table cells, not only Choice's marked spans. Deliberate
  // one-line cuts retain full-value access; this rule rejects wrapping inside an ordinary word.
  const language = document.documentElement.lang;
  const words = new Intl.Segmenter(language === '' ? undefined : language, { granularity: 'word' });
  for (const element of elements) {
    if (element.closest(technical) !== null) continue;
    const text = directText(element);
    const style = styleOf(element);
    if (text === '' || ['nowrap', 'pre'].includes(style.whiteSpace) || losesText(element)) continue;
    const resolvedFont = style.font;
    const rawFont = resolvedFont === '' ? `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}` : resolvedFont;
    let font = wordWidths.fontNames.get(rawFont);
    if (font === undefined) { paint.font = rawFont; font = paint.font; wordWidths.fontNames.set(rawFont, font); }
    const rawSpacing = Number.parseFloat(style.letterSpacing);
    const spacing = Number.isFinite(rawSpacing) ? rawSpacing : 0;
    const byLanguage = bucket(wordWidths.longest, language, () => new Map<string, TransformWidths>());
    const byFont = bucket(byLanguage, font, () => new Map<string, SpacingWidths>());
    const bySpacing = bucket(byFont, style.textTransform, () => new Map<number, TextWidths>());
    const textWidths = bucket(bySpacing, spacing, () => new Map<string, number>());
    const wordsByLanguage = bucket(measuredWords, language, () => new Map<string, TextWidths>());
    const wordWidthsForFont = bucket(wordsByLanguage, font, () => new Map<string, number>());
    let longest = textWidths.get(text);
    if (longest === undefined) {
    const widths = Array.from(words.segment(text)).filter((part) => part.isWordLike).map(({ segment }) => {
      const displayed = style.textTransform === 'uppercase' ? segment.toLocaleUpperCase() : style.textTransform === 'lowercase' ? segment.toLocaleLowerCase() : segment;
      let width = wordWidthsForFont.get(displayed);
      if (width === undefined) { if (paint.font !== font) paint.font = font; width = paint.measureText(displayed).width; wordWidthsForFont.set(displayed, width); }
      return width + Math.max(0, Array.from(displayed).length - 1) * spacing;
    });
    longest = Math.max(0, ...widths);
    textWidths.set(text, longest);
    }
    const box = boxOf(element);
    const available = box.width - Number.parseFloat(style.paddingLeft) - Number.parseFloat(style.paddingRight)
      - Number.parseFloat(style.borderLeftWidth) - Number.parseFloat(style.borderRightWidth);
    // One CSS pixel covers fractional glyph/box rounding; it cannot excuse an 18px option.
    if (longest > available + 1) fail('choice-word-width', text, [element], `Name has ${available.toFixed(2)}px for its longest ${longest.toFixed(2)}px word.`);
  }
  const choiceControl = 'button,a[aria-current="page"],[role="radio"],[role="option"],[role="menuitem"],[role="menuitemradio"],[role="menuitemcheckbox"],[role="tab"],.choice-option';
  const optionControl = '[role="option"],[role="menuitem"],[role="menuitemradio"],[role="menuitemcheckbox"],[cmdk-item]';
  const selectedOrHighlighted = `:is(${choiceControl}):is([data-state="checked"],[data-state="on"],[data-state="active"],[aria-selected="true"],[aria-checked="true"],[aria-pressed="true"],[aria-current="page"],[data-selected="true"],[data-highlighted]),:is(${optionControl}):is(:focus,:hover)`;
  // Match the complex selected/hovered selector once in this rendered state. Ancestor membership
  // is the same closest() relation without reparsing/rematching it for every text node.
  const activeControls = new Set(Array.from(document.querySelectorAll(selectedOrHighlighted)));
  const hasActiveAncestor = (element: Element) => {
    for (let at: Element | null = element; at !== null; at = at.parentElement) if (activeControls.has(at)) return true;
    return false;
  };
  function textPaint(element: HTMLElement): { ink: Colour; ground: Colour } | null {
    let ink = colour(styleOf(element).color);
    let ground: Colour = [0, 0, 0, 0];
    for (let at: Element | null = element; at !== null; at = at.parentElement) {
      const style = styleOf(at), surface = colour(style.backgroundColor);
      if (style.backgroundImage !== 'none') return null;
      ink = over(ink, surface); ground = over(ground, surface);
      const opacity = Number(style.opacity);
      ink = [ink[0], ink[1], ink[2], ink[3] * opacity];
      ground = [ground[0], ground[1], ground[2], ground[3] * opacity];
    }
    return { ink: over(ink, [255, 255, 255, 1]), ground: over(ground, [255, 255, 255, 1]) };
  }
  for (const element of elements) {
    const active = hasActiveAncestor(element);
    const text = directText(element);
    if (!active || text === '' || element.closest('svg,pre,code,input,textarea,[data-user-content]') !== null) continue;
    const rendered = textPaint(element);
    if (rendered === null) {
      fail('choice-text-contrast', text, [element], 'Cannot prove 4.5:1 for selected/highlighted text over an image or gradient; inspect its rendered ground.');
      continue;
    }
    const ratio = contrast(rendered.ink, rendered.ground);
    if (ratio < 4.5) fail('choice-text-contrast', text, [element], `Chosen/highlighted text ${ratio.toFixed(2)}:1; required 4.5:1 on its rendered ground.`);
  }
  if (input.mode === 'choices') return { faults, cuts, allowances, icons: [], menus: [] };
  const boundaryControls = Array.from(new Set([...controls, ...elements.filter((element) => element.matches('[role="radiogroup"],[data-choice-selector]'))]));
  for (const control of boundaryControls) {
    // WCAG 1.4.11 excludes inactive controls; a ghost text/icon action need not invent a box.
    if (control.matches('a,:disabled,[aria-disabled="true"]')) continue;
    // A composed input owns one visible frame around its field and trailing controls.
    const frame = control.matches('input,select,textarea,[role="combobox"]') ? control.closest<HTMLElement>('[data-control-boundary]') ?? control : control;
    const style = styleOf(frame);
    const boundary = ['Top', 'Right', 'Bottom', 'Left'].flatMap((side) => {
      const width = Number.parseFloat(style.getPropertyValue(`border-${side.toLowerCase()}-width`));
      const line = style.getPropertyValue(`border-${side.toLowerCase()}-style`);
      const edge = colour(style.getPropertyValue(`border-${side.toLowerCase()}-color`));
      return width > 0 && line !== 'none' && edge[3] > 0 ? [edge] : [];
    });
    const bounded = control.matches('input:not([type="range"]),select,textarea,[role="radio"],[role="checkbox"],[role="switch"],[role="combobox"]');
    // A segmented option may share its visible frame with its radio group; that frame is checked too.
    if (boundary.length === 0 && control.getAttribute('role') === 'radio' && control.closest('[role="radiogroup"]') !== null && !control.matches('input')) continue;
    if (boundary.length === 0 && !bounded) continue;
    const outside = background(frame.parentElement);
    const inside = background(frame);
    const ratios = [contrast(inside, outside), ...boundary.map((edge) => Math.min(contrast(over(edge, outside), outside), contrast(over(edge, inside), inside)))];
    const best = Math.max(...ratios);
    if (best < 3) fail('control-boundary', nameOf(control), [control], `Enabled control boundary ${best.toFixed(2)}:1; required 3:1 against its adjacent surface.`);
  }

  for (const element of elements) {
    if (element.closest('[role="tooltip"],[data-tooltip-content],input,textarea') !== null) continue;
    const manual = originalValue(element);
    const text = normalize(manual ?? renderedText(element));
    const clipped = losesText(element);
    if (!clipped || text === '') continue;
    const owner = element.closest<HTMLElement>('button,a[href],input,select,textarea,[tabindex],[contenteditable="true"]') ?? element;
    cuts.push({ selector: mark(element), focus: mark(owner), text, title: element.closest('[title]')?.getAttribute('title') ?? null });
    if (owner.tabIndex < 0 && owner.getAttribute('role') !== 'option' && owner.getAttribute('role') !== 'menuitem' && owner.getAttribute('role') !== 'tab') {
      fail('cut-text', text, [element], 'The full value has no keyboard stop or focusable ancestor.');
    }
  }

  // An editor's noneditable toolbar owns ordinary visible labels; its enclosing editable
  // document must not make those labels disappear from their icon-alignment check.
  function firstLine(row: HTMLElement, button: HTMLElement): DOMRect | null {
    const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
      const parent = node.parentElement;
      if (parent === null || button.contains(parent) || parent.closest('[role="tooltip"],svg,input,textarea') !== null || parent.closest('[contenteditable]')?.getAttribute('contenteditable') === 'true' || normalize(node.textContent ?? '') === '' || !visible(parent)) continue;
      const range = document.createRange(); range.selectNodeContents(node);
      const line = Array.from(range.getClientRects()).find((rect) => rect.width > 0 && rect.height > 0);
      if (line !== undefined) return line;
    }
    return null;
  }
  function checkIcons(): void {
  for (const button of controls) {
    const icon = button.querySelector('svg');
    if (icon === null || normalize(renderedText(button)) !== '' || !button.matches('button,[role="button"]') || ['absolute', 'fixed'].includes(styleOf(button).position)) continue;
    for (let row = button.parentElement; row !== null && row !== document.body; row = row.parentElement) {
      const style = styleOf(row);
      // A vertical stack ends the inline row. Its rail/footer cannot align with another pane's heading.
      const flex = style.display === 'flex' || style.display === 'inline-flex';
      const direction = style.flexDirection;
      if (flex && direction.startsWith('column')) break;
      // Independent toolbars own their labels, including a noneditable surface within
      // an editor. Check their own row, but never borrow another surface's text.
      const editorSurface = row.getAttribute('contenteditable') === 'false'
        && row.parentElement?.closest('[contenteditable]')?.getAttribute('contenteditable') === 'true';
      const independent = style.position === 'absolute' || style.position === 'fixed' || editorSurface;
      if (!(flex && !direction.startsWith('column')) && !row.matches('tr,[role="row"]')) {
        if (independent) break;
        continue;
      }
      const line = firstLine(row, button);
      if (line === null) { if (independent) break; continue; }
      const box = boxOf(icon);
      const offset = Math.abs(box.top + box.height / 2 - line.top - line.height / 2);
      if (offset >= 1) fail('icon-centre', nameOf(button), [button, row], `Icon is ${offset.toFixed(2)}px off the first text-line centre; required less than 1px.`);
      break;
    }
  }
  }
  checkIcons();
  // Hidden-on-rest row actions still need geometry evidence after the pointer reveals them.
  let probeSequence = Number(document.documentElement.getAttribute('data-layout-interaction-sequence') ?? '0');
  function interactionProbe(element: HTMLElement, kind: 'icon' | 'menu'): string {
    const attribute = `data-layout-${kind}-probe`;
    let id = element.getAttribute(attribute);
    if (id === null) { probeSequence += 1; id = String(probeSequence); element.setAttribute(attribute, id); }
    return `[${attribute}="${id}"]`;
  }
  const icons = Array.from(document.querySelectorAll('button,[role="button"]')).filter((button): button is HTMLElement =>
    button instanceof HTMLElement && visible(button, true) && button.closest('[role="tooltip"],[data-tooltip-content]') === null &&
    button.querySelector('svg') !== null && normalize(renderedText(button)) === '' && !['absolute', 'fixed'].includes(styleOf(button).position))
    .map((button) => interactionProbe(button, 'icon'));
  const menus = Array.from(document.querySelectorAll('[aria-haspopup="menu"],[aria-haspopup="listbox"],[role="combobox"][aria-haspopup="dialog"]'))
    .filter((element): element is HTMLElement => element instanceof HTMLElement && visible(element, true) && !element.matches(':disabled,[aria-disabled="true"]'))
    .map((element) => interactionProbe(element, 'menu'));
  document.documentElement.setAttribute('data-layout-interaction-sequence', String(probeSequence));
  return { faults, cuts, allowances, icons, menus };
}
