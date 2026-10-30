/** The common safe-link rule used by authored content and the rich-text editor. */
function safeLink(address: string, message: boolean): boolean {
  // Browsers discard these characters while parsing URLs; reject them before classifying a path.
  if (Array.from(address).some((character) => {
    const code = character.charCodeAt(0);
    return code <= 0x1f || code === 0x7f;
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
