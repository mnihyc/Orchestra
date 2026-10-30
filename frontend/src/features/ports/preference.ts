/** The environment view the reader chose, kept by user and environment without content or credentials. */
import { client } from '@/api/client';

function key(id: string): string { return `orch:environment-tab:${client.session.getState().userId ?? 'guest'}:${id}`; }
export function rememberEnvironmentTab(id: string, value: string): void { try { localStorage.setItem(key(id), value); } catch { /* A blocked store keeps the URL choice usable. */ } }
export function preferredEnvironmentTab<T extends string>(id: string, choices: readonly T[]): T | null {
  let value: string | null;
  try { value = localStorage.getItem(key(id)); } catch { return null; }
  return choices.find((choice) => choice === value) ?? null;
}
