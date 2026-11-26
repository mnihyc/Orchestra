/** A successful sign-in can ask for its shell and the first screen of its saved conversation together. */
import type { MeLandingReply, ThreadOpening } from '@/generated/api-types';

let nextUser: string | null = null;
export function expectSignInLanding(user: string): void { nextUser = user; }
export function takeSignInLanding(user: string): boolean {
  const wanted = nextUser === user;
  nextUser = null;
  return wanted;
}

export function rememberedThread(place: { node_id: string; route: string } | null): string | null {
  if (place === null || !/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(place.node_id)) return null;
  return place.route.replace(/\/$/, '') === '/thread/' + place.node_id ? place.node_id : null;
}

// The shell owns the candidate until the landing route selects it. The destination's ordinary
// first read consumes it once, under the same session and active continuation as its native read.
interface LandingReceipt { reply: MeLandingReply; reader: number | undefined }
let receipt: LandingReceipt | null = null;
export function holdLanding(reply: MeLandingReply, reader: number | undefined): void {
  receipt = { reply, reader };
}
export function landingFor(user: string, reader: number | undefined, id: string | null): MeLandingReply['thread'] | undefined {
  const value = receipt;
  receipt = null;
  return value !== null && value.reader === reader && value.reply.shell.person.id === user && value.reply.thread_id === id
    ? value.reply.thread : undefined;
}

interface OfferedThread { user: string; reader: number | undefined; opening: ThreadOpening }
let offered: OfferedThread | null = null;
export function offerLandingThread(user: string, reader: number | undefined, opening: ThreadOpening): void {
  offered = { user, reader, opening };
}
export function visitLanding(path: string): void {
  if (path !== '/' && path !== '/login' && path !== '/login/') {
    nextUser = null;
    receipt = null;
  }
  if (offered !== null && path.replace(/\/$/, '') !== '/thread/' + offered.opening.sync.thread.id) offered = null;
}
export function clearSignInLanding(): void {
  nextUser = null;
  receipt = null;
  offered = null;
}
export function takeLandingThread(user: string | null, reader: number | undefined, id: string): ThreadOpening | null {
  const value = offered;
  if (value?.user !== user || value.reader !== reader || value.opening.sync.thread.id !== id) return null;
  offered = null;
  return value.opening;
}
