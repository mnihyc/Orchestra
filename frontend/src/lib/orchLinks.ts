/** Deployment addresses are identities, never a reason to fetch a foreign site with credentials. */
export interface OrchLinkAddresses {
  public_url?: string;
  alternate_urls?: readonly string[];
  link_endpoints?: readonly { url: string }[];
}

export interface OrchLinkOptions {
  currentUrl?: string;
  addresses?: OrchLinkAddresses;
}

// The API layer supplies its existing cache; no second copy of deployment metadata is kept here.
let source: () => OrchLinkAddresses | undefined = () => undefined;
export function setOrchLinkSource(read: () => OrchLinkAddresses | undefined): void { source = read; }

function currentAddress(): string {
  return typeof location === 'undefined' ? 'https://orch.invalid/' : location.origin;
}

function webAddress(value: string): URL | null {
  if (value.includes('\\') || Array.from(value).some((character) => character.charCodeAt(0) <= 0x20 || character.charCodeAt(0) === 0x7f)) return null;
  try {
    const url = new URL(value);
    return /^(https?:)$/.test(url.protocol) && url.username === '' && url.password === '' ? url : null;
  } catch { return null; }
}

function declaredOrigins(addresses: OrchLinkAddresses): Set<string> {
  const origins = new Set<string>();
  for (const value of [addresses.public_url ?? '', ...addresses.alternate_urls ?? [],
    ...addresses.link_endpoints?.map(({ url }) => url.replace(/^wss:/i, 'https:').replace(/^ws:/i, 'http:')) ?? []]) {
    const url = webAddress(value);
    if (url !== null) origins.add(url.origin);
  }
  return origins;
}

/** Read an application address on this deployment and rewrite it to the address in use.
 * Resource/part parsers still decide which paths and selectors they accept. */
export function readOrchLink(href: string, options: OrchLinkOptions = {}): URL | null {
  const current = webAddress(options.currentUrl ?? currentAddress());
  const clean = href.trim();
  if (current === null || clean.includes('\\') || Array.from(clean).some((character) => character.charCodeAt(0) <= 0x20 || character.charCodeAt(0) === 0x7f)
    || !clean.startsWith('/') && !/^https?:\/\//i.test(clean)) return null;
  try {
    const url = new URL(clean, current);
    if (!/^(https?:)$/.test(url.protocol) || url.username !== '' || url.password !== '') return null;
    const accepted = declaredOrigins(options.addresses ?? source() ?? {});
    accepted.add(current.origin);
    if (!accepted.has(url.origin)) return null;
    return new URL(`${url.pathname}${url.search}${url.hash}`, current.origin);
  } catch { return null; }
}

/** Copy links use the public address; navigation continues on the address already in use. */
export function publicOrchLink(href: string, options: OrchLinkOptions = {}): string {
  const local = readOrchLink(href, options);
  if (local === null) return href;
  const publicUrl = webAddress((options.addresses ?? source())?.public_url ?? '');
  return new URL(`${local.pathname}${local.search}${local.hash}`, publicUrl?.origin ?? local.origin).href;
}
