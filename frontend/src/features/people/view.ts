/** Device-local document view choices, isolated by person and place without stored content or names. */
import { client } from '@/api/client';

const key = (place: string, view: string) => `orch.${client.session.getState().userId ?? 'signed-out'}.document.${place}.${view}`;
export function viewChoice<Value extends string>(place: string, view: string, choices: readonly Value[], fallback: Value): Value {
  try { const saved = localStorage.getItem(key(place, view)); return choices.find((choice) => choice === saved) ?? fallback; }
  catch { return fallback; }
}
export function rememberView(place: string, view: string, choice: string): void {
  try { localStorage.setItem(key(place, view), choice); } catch { /* A device that keeps nothing still changes the view. */ }
}
