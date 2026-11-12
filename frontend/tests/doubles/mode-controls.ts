/** Canonical native mode fixtures follow the generated settings and parameter contracts. */
import type { ResolvedSetting, ThreadParameters } from '@/generated/api-types';

export function nativeModeSetting(key: 'approval.mode' | 'sandbox.mode', value: string, revision = 1, more: Partial<ResolvedSetting> = {}): ResolvedSetting {
  return { key, value, own: { value, revision, locked: false }, source: { scope: 'thread', node: null }, locked_by: null, writable: true, parts: [], ...more };
}
export function modeParameters(mode = 'on_request', more: Partial<ThreadParameters> = {}): ThreadParameters {
  return { model: { value: 'model-1', options: [], writable: true, source: { scope: 'thread', node: null } },
    effort: { value: null, writable: true, supported: false, source: { scope: 'default', node: null } },
    parameters: [], approval_mode: mode, approval_modes: ['ask', 'on_request'], approval_reviewer: null, ...more };
}
