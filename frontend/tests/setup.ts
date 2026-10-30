/** What every unit and component test starts from: the matchers for what is on screen, the English catalogue, and a page that is cleared between tests. */
import '@testing-library/jest-dom/vitest';

import { cleanup } from '@testing-library/react';
import { afterEach, beforeAll, beforeEach, vi } from 'vitest';

import { i18n, startI18n } from '@/i18n';

const geometryRestores: (() => void)[] = [];
const pointerCaptures=new Map<number,Element>();
// jsdom accepts subscriptions; actual geometry and overflow are proved in the browser suite.
beforeEach(() => {
  if (typeof document !== 'undefined' && typeof ResizeObserver === 'undefined') vi.stubGlobal('ResizeObserver', class {
    observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn();
  });
  if (typeof HTMLElement !== 'undefined') {
    // jsdom has no layout. Native detached-anchor handling needs a normal viewport and a
    // visible trigger rectangle for interaction tests; individual geometry spies still win.
    const previousRect = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'getBoundingClientRect');
    Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', { configurable: true, writable: true, value(this: HTMLElement): DOMRect {
      // Read the inherited method now so a test's later Element geometry spy still wins.
      const measured = Element.prototype.getBoundingClientRect.call(this);
      if (measured.width > 0 || measured.height > 0 || !this.matches('[aria-haspopup],[data-state],[data-tooltip-trigger],[data-floating-anchor],[data-radix-select-viewport]')) return measured;
      if (this.closest('[hidden],[style*="display: none"],[style*="visibility: hidden"]') !== null) return measured;
      const style = getComputedStyle(this);
      if (style.display === 'none' || style.visibility === 'hidden') return measured;
      return this.matches('[data-radix-select-viewport]') ? new DOMRect(8, 48, 320, 320) : new DOMRect(16, 64, 120, 32);
    } });
    geometryRestores.push(() => { if (previousRect === undefined) Reflect.deleteProperty(HTMLElement.prototype, 'getBoundingClientRect'); else Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', previousRect); });
    for (const name of ['clientWidth', 'clientHeight'] as const) {
      const previous = Object.getOwnPropertyDescriptor(HTMLElement.prototype, name);
      Object.defineProperty(HTMLElement.prototype, name, { configurable: true, get(this: HTMLElement): number {
        return this.matches('[data-radix-select-viewport]') ? 320 : Reflect.get(Element.prototype, name, this);
      } });
      geometryRestores.push(() => { if (previous === undefined) Reflect.deleteProperty(HTMLElement.prototype, name); else Object.defineProperty(HTMLElement.prototype, name, previous); });
    }
    for (const [name, read] of [['clientWidth', () => window.innerWidth], ['clientHeight', () => window.innerHeight]] as const) {
      const previous = Object.getOwnPropertyDescriptor(document.documentElement, name);
      Object.defineProperty(document.documentElement, name, { configurable: true, get: read });
      geometryRestores.push(() => { if (previous === undefined) Reflect.deleteProperty(document.documentElement, name); else Object.defineProperty(document.documentElement, name, previous); });
    }
  }
  if(typeof Element!=='undefined') {
    // Radix asks for capture state before opening a pointer-driven Select.
    if(typeof Element.prototype.hasPointerCapture!=='function')Object.defineProperty(Element.prototype,'hasPointerCapture',{
      configurable:true,writable:true,value(this:Element,id:number){return pointerCaptures.get(id)===this;},
    });
    if(typeof Element.prototype.setPointerCapture!=='function')Object.defineProperty(Element.prototype,'setPointerCapture',{
      configurable:true,writable:true,value(this:Element,id:number){pointerCaptures.set(id,this);},
    });
    if(typeof Element.prototype.releasePointerCapture!=='function')Object.defineProperty(Element.prototype,'releasePointerCapture',{
      configurable:true,writable:true,value(this:Element,id:number){if(pointerCaptures.get(id)===this)pointerCaptures.delete(id);},
    });
    // There is no layout engine here; real scrolling remains a browser-suite assertion.
    if(typeof Element.prototype.scrollIntoView!=='function')Object.defineProperty(Element.prototype,'scrollIntoView',{
      configurable:true,writable:true,value:()=>undefined,
    });
  }
});

beforeAll(async () => {
  if (!i18n.isInitialized) await startI18n('en');
  const { installDevCatalogue } = await import('@/i18n/dev');
  installDevCatalogue();
});

afterEach(() => {
  // A test of something that needs no page runs without one, and has nothing to clear.
  if (typeof document === 'undefined') return;
  cleanup();
  for (const restore of geometryRestores.splice(0).reverse()) restore();
  pointerCaptures.clear();
  localStorage.clear();
});
