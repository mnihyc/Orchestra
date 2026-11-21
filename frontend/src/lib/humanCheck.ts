/** The vendors of the human check, as a page loads and drives them.
 *
 * Nothing here runs unless the system says, in what it tells everybody of itself, that it asks a human
 * check; and the screens that call it are only the ones where a person without a session signs in,
 * registers or has mail sent. The vendor's script is added to the page when one of those screens needs
 * it, once, and never otherwise. A vendor's script cannot be taken away again, so it stays until the
 * page is reloaded.
 *
 * The two vendors differ in what the page does and not in what it is told: hCaptcha draws a box
 * (or runs unseen and asks only when it doubts), and reCAPTCHA of Google Cloud Fraud Defense always
 * runs unseen and answers with a token made for the act. Both give a token, which the screen sends
 * in the `X-Human-Check` header of its one request and then counts as spent.
 */
import type { HumanCheckAct, MetaHumanCheck } from '@/generated/api-types';

/** What a screen draws and asks for, whichever vendor stands behind it. */
export interface Vendor {
  /** Draws the checkbox into `container`; `on` is told what the person does with it. */
  render(container: HTMLElement, on: Callbacks): Drawn;
  /** Runs unseen and gives a token for `act`; rejects when the vendor cannot be reached. */
  run(act: HumanCheckAct): Promise<string>;
}

/** What the widget tells the screen. */
export interface Callbacks {
  /** The person ticked the box, or the vendor passed them: here is the token. */
  token(token: string): void;
  /** The token ran out before it was used, or the challenge was left. */
  expired(): void;
  /** The vendor could not go on. */
  failed(): void;
}

/** A widget that was drawn. */
export interface Drawn {
  /** Makes it fresh again, as before it was ticked. */
  reset(): void;
  /** Takes it out of its container. */
  dispose(): void;
}

/** The word a vendor's unseen run is refused with. */
export class VendorUnavailable extends Error {
  constructor(message = 'the human check could not be reached') {
    super(message);
    this.name = 'VendorUnavailable';
  }
}

/**
 * What a screen sends in the place of a token when the vendor's widget could not be loaded or run in this browser:
 * the word says so, and the server looks into it for itself (anybody could say it), and lets the person in or turns
 * them away as it does when the service cannot be reached. The same word as the server's `HUMAN_CHECK_UNLOADED`.
 */
export const UNLOADED = 'unloaded';

/** The one name by which a vendor is asked for a token made for an act: letters and underscores, the same for everybody. */
export const vendorAction = (act: HumanCheckAct): string => `orch_${act}`;

/** How long a page waits for the service's widget: the system's setting, in milliseconds. */
function waitMs(check: MetaHumanCheck): number {
  return check.wait_secs * 1000;
}

/** Below this width the box is drawn in its compact size: the normal one is 303 pixels wide. */
const NORMAL_WIDTH_PX = 303;

interface HcaptchaApi {
  render(container: HTMLElement | string, params: Record<string, unknown>): string;
  reset(id?: string): void;
  execute(id?: string, options?: { async: boolean }): Promise<{ response: string; key: string }> | undefined;
}

interface GrecaptchaApi {
  ready(callback: () => void): void;
  execute(siteKey: string, options: { action: string }): Promise<string>;
}

interface Grecaptcha extends GrecaptchaApi {
  enterprise?: GrecaptchaApi;
}

declare global {
  interface Window {
    hcaptcha?: HcaptchaApi;
    grecaptcha?: Grecaptcha;
    orchHcaptchaLoaded?: () => void;
  }
}

const scripts = new Map<string, Promise<void>>();

/** Adds a vendor's script to the page once, and says when it has run; rejects if the page cannot load it. */
function addScript(address: string, ready?: () => boolean): Promise<void> {
  const held = scripts.get(address);
  if (held !== undefined) return held;
  const loaded = new Promise<void>((resolve, reject) => {
    const script = document.createElement('script');
    script.src = address;
    script.async = true;
    script.defer = true;
    script.onload = () => {
      if (ready === undefined || ready()) resolve();
      else reject(new VendorUnavailable('the script of the human check did not start'));
    };
    script.onerror = () => {
      scripts.delete(address);
      script.remove();
      reject(new VendorUnavailable('the script of the human check could not be loaded'));
    };
    document.head.append(script);
  });
  scripts.set(address, loaded);
  return loaded;
}

/** Makes a promise give up after `ms`. */
function within<T>(promise: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new VendorUnavailable('the human check took too long')), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error instanceof Error ? error : new VendorUnavailable());
      },
    );
  });
}

function isDark(): boolean {
  return document.documentElement.dataset.theme === 'dark';
}

// ---------------------------------------------------------------- hCaptcha

async function hcaptcha(check: MetaHumanCheck, locale: string): Promise<Vendor> {
  const address = `https://js.hcaptcha.com/1/api.js?render=explicit&hl=${encodeURIComponent(locale)}&onload=orchHcaptchaLoaded`;
  if (window.hcaptcha === undefined) {
    const ready = new Promise<void>((resolve) => {
      window.orchHcaptchaLoaded = resolve;
    });
    await addScript(address);
    await within(ready, waitMs(check));
  }
  const api = window.hcaptcha;
  if (api === undefined) throw new VendorUnavailable();
  let unseen: string | null = null;
  return {
    render(container, on) {
      const width = container.getBoundingClientRect().width;
      const id = api.render(container, {
        sitekey: check.site_key,
        theme: isDark() ? 'dark' : 'light',
        size: width > 0 && width < NORMAL_WIDTH_PX ? 'compact' : 'normal',
        callback: (token: string) => on.token(token),
        'expired-callback': () => on.expired(),
        'chalexpired-callback': () => on.expired(),
        'error-callback': () => on.failed(),
      });
      return { reset: () => api.reset(id), dispose: () => container.replaceChildren() };
    },
    async run() {
      if (unseen === null) {
        const holder = document.createElement('div');
        document.body.append(holder);
        unseen = api.render(holder, { sitekey: check.site_key, size: 'invisible', theme: isDark() ? 'dark' : 'light' });
      }
      const id = unseen;
      const answer = api.execute(id, { async: true });
      if (answer === undefined) throw new VendorUnavailable();
      try {
        const { response } = await within(answer, waitMs(check));
        return response;
      } finally {
        api.reset(id);
      }
    },
  };
}

// ---------------------------------------------------------------- reCAPTCHA

async function recaptcha(check: MetaHumanCheck, locale: string): Promise<Vendor> {
  const host = check.alternate_host ? 'www.recaptcha.net' : 'www.google.com';
  const enterprise = check.verification === 'assessments';
  const script = enterprise ? 'enterprise.js' : 'api.js';
  const address = `https://${host}/recaptcha/${script}?render=${encodeURIComponent(check.site_key)}&hl=${encodeURIComponent(locale)}`;
  const pick = (): GrecaptchaApi | undefined => (enterprise ? window.grecaptcha?.enterprise : window.grecaptcha);
  await addScript(address, () => pick() !== undefined);
  const grecaptcha = pick();
  if (grecaptcha === undefined) throw new VendorUnavailable();
  return {
    render() {
      // A score-based key has no box: nothing is drawn.
      return { reset: () => undefined, dispose: () => undefined };
    },
    async run(act) {
      await within(new Promise<void>((resolve) => grecaptcha.ready(resolve)), waitMs(check));
      return within(grecaptcha.execute(check.site_key, { action: vendorAction(act) }), waitMs(check));
    },
  };
}

/** Loads the vendor the system chose and returns what a screen drives; rejects when it cannot be loaded. */
export function loadVendor(check: MetaHumanCheck, locale: string): Promise<Vendor> {
  return check.provider === 'hcaptcha' ? hcaptcha(check, locale) : recaptcha(check, locale);
}
