/** The route suite's browser evidence: actual accessible names and whole cut values by both inputs. */
import { expect, type CDPSession, type Page } from '@playwright/test';
import { inspectLayout, prepareLayoutPointer, trackLayoutPointer, type LayoutPointer, type DuplicateAllowance, type LayoutFault, type LayoutInput, type LayoutResult } from './layout-rules-dom.ts';
import { RAW_VALUES } from './layout-routes.ts';

/** No duplicate-text exceptions are currently needed. Any future exception must name exact sites and a reason. */
export const DUPLICATE_ALLOW_LIST: DuplicateAllowance[] = [];
export const FULL_VALUE_DISCLOSURE = { reason: 'A full-value tooltip repeats its own actually cut value; this exception verifies a single original, real clipping and ARIA ownership.' };
export const LAYOUT_INPUT: LayoutInput = { duplicateAllowList: DUPLICATE_ALLOW_LIST, fullValueDisclosure: FULL_VALUE_DISCLOSURE, rawValues: RAW_VALUES };
/** Five ordinary tooltip-rest intervals tolerate scheduling without silently waiting a whole test. */
const REVEAL_DEADLINE_MS = 2_000;

// Transient probes need browser input and rendered measurements, not another full trace DOM snapshot
// for each option. Keep one native Chromium session; the complete page pass and screen attachments stay.
const layoutSessions = new WeakMap<Page, Promise<CDPSession>>();
function layoutSession(page: Page): Promise<CDPSession> {
  let session = layoutSessions.get(page);
  if (session === undefined) { session = page.context().newCDPSession(page); layoutSessions.set(page, session); }
  return session;
}
async function measureInBrowser<Argument, Result>(page: Page, fn: (argument: Argument) => Result | Promise<Result>, argument: Argument): Promise<Result> {
  const session = await layoutSession(page);
  const result = await session.send('Runtime.evaluate', {
    expression: `(${fn.toString()})(${JSON.stringify(argument)})`, awaitPromise: true, returnByValue: true,
  });
  if (result.exceptionDetails !== undefined) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  return result.result.value as Result;
}
export async function pressLayoutKey(page: Page, key: 'Home' | 'ArrowDown' | 'Escape'): Promise<void> {
  const session = await layoutSession(page);
  const value = key === 'Home' ? 36 : key === 'ArrowDown' ? 40 : 27;
  // Ordered protocol messages deliver the same trusted down/up without waiting on an
  // extra driver acknowledgement between them. The measurement still waits for the
  // real keyup and two render frames; every option and pointer state is retained.
  await Promise.all([
    session.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code: key, windowsVirtualKeyCode: value, nativeVirtualKeyCode: value }),
    session.send('Input.dispatchKeyEvent', { type: 'keyUp', key, code: key, windowsVirtualKeyCode: value, nativeVirtualKeyCode: value }),
  ]);
}

/** Install code once per transient walk; every invocation still reads the current DOM and styles. */
interface InputCounts { key: number; pointer: number }
async function installLayoutProbe(page: Page): Promise<InputCounts> {
  const session = await layoutSession(page);
  // Reuse executable code across menus in the same document. Replacing the function
  // after every menu discards its compiled state; all rendered measurements stay fresh.
  const existing = await session.send('Runtime.evaluate', { returnByValue: true, expression: `globalThis.__orchLayoutProbeRoot===document.documentElement?({key:globalThis.__layoutPointer.keySerial,pointer:globalThis.__layoutPointer.pointerSerial}):null` });
  if (existing.result.value !== null) return existing.result.value as InputCounts;
  const installed = await session.send('Runtime.evaluate', { returnByValue: true, expression: `globalThis.__orchLayoutInspect=(${inspectLayout.toString()});globalThis.__orchLayoutPrepare=(${prepareLayoutPointer.toString()});globalThis.__orchLayoutInput=${JSON.stringify(LAYOUT_INPUT)};globalThis.__orchLayoutProbeRoot=document.documentElement;({key:globalThis.__layoutPointer.keySerial,pointer:globalThis.__layoutPointer.pointerSerial})` });
  return installed.result.value as InputCounts;
}
interface TransientResult { result: LayoutResult; next: LayoutPointer | null; inputs: InputCounts }
async function transientLayout(page: Page, mode: 'choices' | 'icons', pointerTarget?: string, next: string[] = [], afterInput?: LayoutInput['afterInput']): Promise<TransientResult> {
  const session = await layoutSession(page);
  const options = JSON.stringify({ mode, settle: true, pointerTarget, afterInput });
  const reply = await session.send('Runtime.evaluate', { expression: `(async()=>{const result=await globalThis.__orchLayoutInspect({...globalThis.__orchLayoutInput,...${options}});return {result,next:globalThis.__orchLayoutPrepare(${JSON.stringify(next)}),inputs:{key:globalThis.__layoutPointer.keySerial,pointer:globalThis.__layoutPointer.pointerSerial}}})()`, awaitPromise: true, returnByValue: true });
  if (reply.exceptionDetails !== undefined) throw new Error(reply.exceptionDetails.exception?.description ?? reply.exceptionDetails.text);
  return reply.result.value as TransientResult;
}
async function pointerWalk(page: Page, mode: 'choices' | 'icons', selectors: string[], initial: LayoutPointer | null, initialInputs: InputCounts): Promise<LayoutFault[]> {
  const faults: LayoutFault[] = [];
  let target = initial;
  let inputs = initialInputs;
  while (target !== null) {
    const index = selectors.indexOf(target.selector), remaining = selectors.slice(index + 1);
    if (target.tooltips > 0) {
      const refreshed = await dismissTooltips(page, target.selector);
      if (refreshed === false) { target = await measureInBrowser(page, prepareLayoutPointer, remaining); continue; }
      target = { ...target, ...refreshed };
    }
    const moved = hoverMeasured(page, target);
    const [, measured] = await Promise.all([moved, transientLayout(page, mode, target.selector, remaining, { kind: 'pointer', sequence: inputs.pointer })]);
    faults.push(...measured.result.faults); target = measured.next; inputs = measured.inputs;
  }
  return faults;
}

/** A tooltip from the preceding probe must not cover the next control's real pointer target. */
async function dismissTooltips(page: Page, selector?: string): Promise<{ x: number; y: number } | false> {
  // Read the target and existing disclosures together. With no tooltip, moving out and back
  // only repeats input work; the next native hover still proves its actual trusted hit target.
  const state = await measureInBrowser(page, (selector: string | undefined) => {
    const target = selector === undefined ? null : document.querySelector(selector);
    target?.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' });
    const box = target?.getBoundingClientRect();
    return { tooltips: document.querySelectorAll('[role="tooltip"]').length, x: box === undefined ? 0 : box.left + box.width / 2, y: box === undefined ? 0 : box.top + box.height / 2,
      visible: selector === undefined || (target !== null && box !== undefined && box.width > 0 && box.height > 0 && getComputedStyle(target).visibility !== 'hidden') };
  }, selector);
  if (!state.visible) return false;
  if (state.tooltips > 0) {
    await pressLayoutKey(page, 'Escape');
    await expect(page.getByRole('tooltip')).toHaveCount(0);
  } else expect(state.tooltips).toBe(0);
  return { x: state.x, y: state.y };
}

/** The measurement proves the actual pointer hit after its two render frames. Repeated menu probes
 * need no separate stabilization wait before that checked native pointer action.
 */
async function hoverMeasured(page: Page, point: { x: number; y: number }): Promise<void> {
  const session = await layoutSession(page);
  await session.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: point.x, y: point.y, buttons: 0, pointerType: 'mouse' });
}

/** Chrome's accessibility tree supplies the browser's real computed names, not an approximation from aria-label. */
export async function duplicateAccessibleNames(page: Page): Promise<LayoutFault[]> {
  const session = await page.context().newCDPSession(page);
  try {
    const { nodes } = await session.send('Accessibility.getFullAXTree');
    const byId = new Map(nodes.map((node) => [node.nodeId, node]));
    // Computed headings can be authored data. Resolve their actual content role, never
    // their spelling: source/file bodies are exempt; document headings compare within
    // their real block. All interactive controls keep the ordinary native-context check.
    const authored = new Map<number, { source: boolean; block: number }>();
    // The page and each transient layer are read separately. Native layer roles and
    // shared floating content identify that boundary; no word grants an exemption.
    // Existing row/landmark context still rejects duplicate controls within a layer.
    const layers = new Map<number, number>();
    if (await page.locator('[data-content-role],[role="dialog"],[role="alertdialog"],[role="menu"],dialog[open],.floating-layer').count() > 0) {
      const dom = await session.send('DOM.getDocument', { depth: -1, pierce: true });
      type DomNode = typeof dom.root;
      function visit(node: DomNode, source = false, documentRoot = 0, block = 0, toolbar = false, control = false, layer = 0): void {
        const attrs = new Map<string, string>();
        for (let index = 0; index < (node.attributes?.length ?? 0); index += 2) attrs.set(node.attributes![index]!, node.attributes![index + 1]!);
        if (['dialog', 'alertdialog', 'menu'].includes(attrs.get('role') ?? '') || node.nodeName === 'DIALOG' && attrs.has('open') || (attrs.get('class') ?? '').split(/\s+/).includes('floating-layer')) layer = node.backendNodeId;
        if (layer !== 0) layers.set(node.backendNodeId, layer);
        const content = attrs.get('data-content-role');
        if (content === 'source' || content === 'document') { source = content === 'source'; documentRoot = content === 'document' ? node.backendNodeId : 0; block = documentRoot; toolbar = false; control = false; }
        else {
          if (attrs.get('contenteditable') === 'false') toolbar = true;
          else if (attrs.get('contenteditable') === 'true') toolbar = false;
          control ||= ['BUTTON', 'A', 'INPUT', 'SELECT', 'TEXTAREA', 'SUMMARY'].includes(node.nodeName) || ['button', 'tab', 'radio', 'checkbox', 'switch', 'combobox', 'textbox', 'searchbox', 'menuitem'].includes(attrs.get('role') ?? '');
        }
        if (documentRoot !== 0 && ['block', 'leaf'].includes(attrs.get('data-ob') ?? '')) block = node.backendNodeId;
        if ((/^H[1-6]$/.test(node.nodeName) || attrs.get('role') === 'heading') && !control && !toolbar && (source || documentRoot !== 0)) authored.set(node.backendNodeId, { source, block });
        for (const child of [...(node.children ?? []), ...(node.shadowRoots ?? [])]) visit(child, source, documentRoot, block, toolbar, control, layer);
      }
      visit(dom.root);
    }
    const word = (value: unknown): string => typeof value === 'string' ? value.replace(/\s+/g, ' ').trim() : '';
    const landmarks = new Set(['banner', 'complementary', 'contentinfo', 'form', 'main', 'navigation', 'region', 'search', 'dialog', 'alertdialog']);
    // A native row supplies the context a reader gets, including its cell/column headers
    // (WCAG2.4.4). The same person's chip across rows keeps the same name and target,
    // as consistent identification requires (WCAG3.2.4). Duplicates inside one row or
    // cell, including the same name for different targets, still fail. Ordinary controls
    // outside repeated rows retain their nearest-landmark comparison.
    const rows = new Set(['row', 'listitem', 'treeitem', 'option', 'menuitem', 'menuitemradio', 'menuitemcheckbox']);
    const namedRoles = new Set(['button', 'togglebutton', 'popupbutton', 'menubutton', 'link', 'combobox', 'checkbox', 'radio', 'switch', 'tab', 'textbox', 'searchbox', 'heading', 'menuitem', 'menuitemradio', 'menuitemcheckbox']);
    type AccessibleNode = (typeof nodes)[number];
    interface NameGroup { name: string; landmark: string; nodes: AccessibleNode[] }
    const grouped = new Map<string, NameGroup>();
    function contains(ancestor: string, child: AccessibleNode): boolean {
      let parent = child.parentId === undefined ? undefined : byId.get(child.parentId);
      while (parent !== undefined) {
        if (parent.nodeId === ancestor) return true;
        parent = parent.parentId === undefined ? undefined : byId.get(parent.parentId);
      }
      return false;
    }
    for (const node of nodes) {
      const role = word(node.role?.value).toLocaleLowerCase(), name = word(node.name?.value);
      if (node.ignored || name === '' || !namedRoles.has(role)) continue;
      const content = role === 'heading' && node.backendDOMNodeId !== undefined ? authored.get(node.backendDOMNodeId) : undefined;
      if (content?.source === true) continue;
      let parent = node.parentId === undefined ? undefined : byId.get(node.parentId);
      while (parent !== undefined && !landmarks.has(word(parent.role?.value)) && !rows.has(word(parent.role?.value))) parent = parent.parentId === undefined ? undefined : byId.get(parent.parentId);
      const landmark = content !== undefined ? `authored block ${content.block}` : parent === undefined ? 'document' : `${word(parent.role?.value)} ${word(parent.name?.value)} [AX ${parent.nodeId}]`;
      const layer = node.backendDOMNodeId === undefined ? 0 : layers.get(node.backendDOMNodeId) ?? 0;
      const key = `${layer}:${content === undefined ? parent?.nodeId ?? 'document' : `authored:${content.block}`}:${name}`;
      const group: NameGroup = grouped.get(key) ?? { name, landmark, nodes: [] };
      // A heading containing its own link/button is one rendered occurrence, not two controls.
      if (group.nodes.some((prior) => contains(node.nodeId, prior))) continue;
      group.nodes = group.nodes.filter((prior) => !contains(prior.nodeId, node));
      group.nodes.push(node); grouped.set(key, group);
    }
    return [...grouped.values()].filter((group) => group.nodes.length > 1).map((group) => ({ rule: 'duplicate-name', text: group.name,
      selectors: group.nodes.map((node) => `AX ${node.nodeId} ${word(node.role?.value)}`), detail: `The same computed accessible name appears ${group.nodes.length} times inside ${group.landmark}.` }));
  } finally { await session.detach(); }
}

/** Loading indicators cannot count as a successful screen walk. No swallowed waits or networkidle on a live app. */
export async function screenReady(page: Page, locale: string, theme: string): Promise<void> {
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  await expect(page.locator('#root')).not.toBeEmpty();
  await expect(page.getByRole('heading').first()).toBeVisible();
  await expect(page.locator('[aria-busy="true"]:visible')).toHaveCount(0);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done())));
  });
}

export async function auditPage(page: Page): Promise<LayoutResult> {
  const result = await page.evaluate(inspectLayout, LAYOUT_INPUT);
  result.faults.push(...await duplicateAccessibleNames(page));
  for (const cut of result.cuts) {
    try {
      await dismissTooltips(page);
      const source = page.locator(cut.selector);
      await source.hover();
      if (!cut.title?.replace(/\s+/g, ' ').includes(cut.text)) {
        await expect.poll(() => page.locator('[role="tooltip"]').allTextContents().then((texts) => texts.map((text) => text.replace(/\s+/g, ' ').trim()).join('\n')),
          { timeout: REVEAL_DEADLINE_MS, message: `Hover must reveal ${cut.text}` }).toContain(cut.text);
      }
      // Locator.focus calls this same native method. Batch its observations through the
      // existing browser session, retaining both real focus and its owned full-value hint.
      await measureInBrowser(page, (selector: string) => {
        const owner = document.querySelector<HTMLElement>(selector);
        if (owner === null) throw new Error('The cut value lost its focus owner.');
        owner.focus();
      }, cut.focus);
      await expect.poll(() => measureInBrowser(page, (selector: string) => {
        const owner = document.querySelector(selector);
        return { focused: owner !== null && document.activeElement === owner,
          text: (owner?.getAttribute('aria-describedby') ?? '').split(/\s+/)
            .map((id) => document.getElementById(id)).filter((description): description is HTMLElement => description?.getAttribute('role') === 'tooltip')
            .map((description) => description.textContent.replace(/\s+/g, ' ').trim()).join('\n') };
      }, cut.focus), { timeout: REVEAL_DEADLINE_MS, message: `Keyboard focus must reveal ${cut.text} on its actual owner` })
        .toEqual({ focused: true, text: expect.stringContaining(cut.text) });
    } catch (error) {
      result.faults.push({ rule: 'cut-text', text: cut.text, selectors: [cut.selector, cut.focus], detail: error instanceof Error ? error.message : String(error) });
    }
  }
  await page.mouse.move(0, 0);
  return result;
}

/** Open every currently visible menu/listbox without selecting an act. No guessed per-route menu sample. */
export async function auditOpenMenus(page: Page, selectors: string[]): Promise<LayoutFault[]> {
  const faults: LayoutFault[] = [];
  for (const selector of selectors) {
    const trigger = page.locator(selector);
    if (!await dismissTooltips(page, selector)) continue;
    await trigger.click();
    await expect(page.locator('[role="menu"]:visible,[role="listbox"]:visible').first()).toBeVisible();
    faults.push(...(await auditPage(page)).faults);
    faults.push(...await auditHighlightedOptions(page));
    // Close only the open pickers, including a submenu exposed by hover. An extra Escape may
    // dismiss a tooltip; never keep sending Escape into the underlying dialog after they close.
    const layers = page.locator(':is([role="menu"],[role="listbox"]):visible:not([data-state="closed"],[data-state="closed"] *)');
    const limit = await layers.count() + 1;
    for (let attempt = 0; attempt < limit && await layers.count() > 0; attempt += 1) await pressLayoutKey(page, 'Escape');
    await expect(page.locator('[role="menu"]:visible,[role="listbox"]:visible')).toHaveCount(0);
  }
  return faults;
}

/** Exercise transient selected text using real navigation and hover, never Enter/click on an act. */
export async function auditHighlightedOptions(page: Page): Promise<LayoutFault[]> {
  await page.evaluate(trackLayoutPointer);
  const selectors = await page.locator('[role="option"]:visible,[role="menuitem"]:visible,[role="menuitemradio"]:visible,[role="menuitemcheckbox"]:visible,[cmdk-item]:visible').evaluateAll((options) => {
    let sequence = Number(document.documentElement.getAttribute('data-layout-option-sequence') ?? '0');
    const selectors = options.map((option) => {
      let id = option.getAttribute('data-layout-option-probe');
      if (id === null) { sequence += 1; id = String(sequence); option.setAttribute('data-layout-option-probe', id); }
      return `[data-layout-option-probe="${id}"]`;
    });
    document.documentElement.setAttribute('data-layout-option-sequence', String(sequence));
    return selectors;
  });
  const faults: LayoutFault[] = [];
  if (selectors.length === 0) return faults;
  let inputs = await installLayoutProbe(page);
  let first: LayoutPointer | null = null;
  for (const [step] of selectors.entries()) {
    const pressed = pressLayoutKey(page, step === 0 ? 'Home' : 'ArrowDown');
    const [, measured] = await Promise.all([pressed, transientLayout(page, 'choices', undefined, step === selectors.length - 1 ? selectors : [], { kind: 'key', sequence: inputs.key })]);
    faults.push(...measured.result.faults); first = measured.next; inputs = measured.inputs;
  }
  faults.push(...await pointerWalk(page, 'choices', selectors, first, inputs));
  return faults;
}

/** CSS opacity on a row's resting actions cannot hide a misplaced icon from the suite. */
export async function auditHoverIcons(page: Page, selectors: string[]): Promise<LayoutFault[]> {
  await page.evaluate(trackLayoutPointer);
  const inputs = await installLayoutProbe(page);
  const first = await measureInBrowser(page, prepareLayoutPointer, selectors);
  return pointerWalk(page, 'icons', selectors, first, inputs);
}
