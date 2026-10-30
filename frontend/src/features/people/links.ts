/** The common safe-link rule used by authored content and the rich-text editor. */
function safeLink(address: string, message: boolean): boolean {
  // Reject controls before classifying a path; browsers discard tabs and line breaks while parsing URLs.
  if (Array.from(address).some((character) => {
    const code = character.charCodeAt(0);
    return code <= 0x1f || (code >= 0x7f && code <= 0x9f);
  })) return false;
  const clean = address.trim();
  const scheme = message ? /^(https?:|mailto:)/i : /^(https?:\/\/|mailto:)/i;
  return scheme.test(clean) || /^(\/(?![/\\])|#)/.test(clean) || (message && /^\.\.?\//.test(clean));
}

/** Whether an editor link leads to a web address, mail, or a path of this application. */
export function linkAllowed(address: string): boolean {
  return safeLink(address, false);
}

/** Whether a message link leads to a web address, mail, or a local or relative path. */
export function messageLinkAllowed(address: string): boolean {
  return safeLink(address, true);
}

/** Resolves browser-normalized paths explicitly so external links open separately. */
export function messageLinkTarget(address: string, allowProtocolRelative = false): string | null {
  if (messageLinkAllowed(address)) return address;
  const clean = address.trim();
  if (!/^(\/|#|\.\.?\/)/.test(clean) || (!allowProtocolRelative && clean.startsWith('//'))) return null;
  try {
    const target = new URL(clean, location.href);
    if (target.protocol !== 'http:' && target.protocol !== 'https:') return null;
    return target.origin === location.origin ? `${target.pathname}${target.search}${target.hash}` : target.href;
  } catch {
    return null;
  }
}
