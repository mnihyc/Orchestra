/** Load the emitted application once its shell has painted, without fetching a redundant facade. */
import { pageHtmlReads } from '@/api/htmlReads';
import type { Locale } from '@/generated/api-types';
import { applicationInitialization } from './applicationInitialization';

const styles = new Map<string, Promise<void>>();
const startApplication = applicationInitialization((href) => import(/* @vite-ignore */ href));

function ownUrl(file: string): string {
  const url = new URL(`/${file}`, location.href);
  if (url.origin !== location.origin) throw new Error('Application files must belong to this page.');
  return url.href;
}

function stylesheet(href: string): Promise<void> {
  const held = styles.get(href);
  if (held !== undefined) return held;
  const existing = [...document.querySelectorAll<HTMLLinkElement>('link[rel="stylesheet"]')].find((link) => link.href === href);
  if (existing?.sheet !== null && existing?.sheet !== undefined) return Promise.resolve();
  const link = existing ?? document.createElement('link');
  const pending = new Promise<void>((resolve, reject) => {
    const clean = () => { link.removeEventListener('load', loaded); link.removeEventListener('error', failed); };
    const loaded = () => { clean(); resolve(); };
    const failed = () => { clean(); styles.delete(href); link.remove(); reject(new Error('The application stylesheet could not load.')); };
    link.addEventListener('load', loaded); link.addEventListener('error', failed);
    // Anonymous, as the document's own stylesheets, the bundler's and the starter's preloads ask them: one fetch each.
    if (existing === undefined) { link.rel = 'stylesheet'; link.crossOrigin = 'anonymous'; link.href = href; document.head.appendChild(link); }
  });
  styles.set(href, pending);
  return pending;
}

// The sign-in document knows its two public catalogues. Start both in the device's language
// while the application is loading, so neither rendering the shell nor entering the form needs another flight.
export function preloadLoginWords(locale: Locale): void {
  const words = document.querySelector<HTMLMetaElement>('meta[name="orch-login-words"]');
  if (words === null) return;
  for (const part of words.content.split(' ')) {
    const link = document.createElement('link');
    link.rel = 'preload';
    link.as = 'fetch';
    link.crossOrigin = 'anonymous';
    link.href = ownUrl(`assets/words/${locale}-${part}.json`);
    document.head.append(link);
  }
}

/** Fonts cannot occupy the connections needed to paint the first shell or dispatch its code. */
function preloadFonts(): void {
  for (const link of document.querySelectorAll<HTMLLinkElement>('link[data-orch-font]')) link.media = 'all';
}

export async function loadApplication(): Promise<unknown> {
  const application = pageHtmlReads()?.application;
  if (application === undefined) return import('@/application');
  const href = ownUrl(application.file);
  if (![...document.querySelectorAll<HTMLLinkElement>('link[rel="modulepreload"]')].some((link) => link.href === href)) {
    const link = document.createElement('link');
    link.rel = 'modulepreload'; link.href = href; link.crossOrigin = 'anonymous';
    document.head.appendChild(link);
  }
  const ready = Promise.all(application.styles.map((file) => stylesheet(ownUrl(file))));
  preloadFonts();
  await ready;
  return startApplication(href, application.initializers);
}
