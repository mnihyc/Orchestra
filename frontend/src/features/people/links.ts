/** The common safe-link rule used by inert writing conversion and the rich-text editor. */
export function linkAllowed(address: string): boolean {
  return /^(https?:\/\/|mailto:|\/(?!\/)|#)/i.test(address.trim());
}
