/** Built screens offered by navigation; direct addresses retain their truthful unavailable state. */
// Management is built but for the sections whose part is not joined yet (the same as `features/admin/sections.ts` leaves unlisted).
const UNBUILT = [/^\/admin\/(?:email|templates|mcp|defaults|rates)(?:\/|$)/];

/** A registered route is offered only when its body is built. */
export function screenBuilt(href: string): boolean {
  return !UNBUILT.some((pattern) => pattern.test(href.split('?')[0] ?? href));
}
