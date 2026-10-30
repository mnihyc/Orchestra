/** Contract-wide, bilingual error coverage: no protocol token is a person's message. */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterAll, beforeAll, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type { ErrorCode } from '@/generated/api-types';
import { i18n, setLocale, startI18n } from '@/i18n';
import { apiErrorSentence, fieldErrorSentence, hasErrorCode } from '@/i18n/errors';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';
import { errorText } from '@/ui/errorText';

// A module-relative filesystem path also works in jsdom; Vite rewrites new URL assets to HTTP.
const source = readFileSync(resolve(import.meta.dirname, '../../../crates/contracts/api/src/common.rs'), 'utf8');
const codes = [...source.slice(source.indexOf('error_codes! {')).matchAll(/^\s+\w+\s*=\s*\d+,\s*"([^"]+)"/gmu)]
  .flatMap((match) => match[1] === undefined ? [] : [match[1]]);
const diagnostic = 'harness_unavailable: agent.harnesses: model_inputs_segment_id_fkey; threads_starting_step_check; store_unavailable';

function sentence(catalogue: unknown, code: string): string {
  let value: unknown = catalogue;
  for (const part of ['errors', ...code.split('.')]) {
    if (typeof value !== 'object' || value === null) throw new Error(`Missing error words for ${code}`);
    value = (value as Record<string, unknown>)[part];
  }
  if (typeof value !== 'string') throw new Error(`Missing error words for ${code}`);
  return value;
}

beforeAll(async () => { await startI18n('en'); });
afterAll(async () => { await setLocale('en'); });

it('keeps the environment explanation while translations are loading', () => {
  const initialized = i18n.isInitialized;
  try {
    i18n.isInitialized = false;
    expect(fieldErrorSentence('the selected environment is unavailable', undefined, 'environment.use'))
      .toBe('the selected environment is unavailable');
  } finally {
    i18n.isInitialized = initialized;
  }
});

it('keeps the environment explanation until its catalogue arrives', () => {
  const resource = vi.spyOn(i18n, 'getResource').mockReturnValueOnce(undefined);
  try {
    expect(fieldErrorSentence('the selected environment is unavailable', undefined, 'environment.use'))
      .toBe('the selected environment is unavailable');
  } finally {
    resource.mockRestore();
  }
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`explains an unavailable selected environment in ${locale}`, async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    for (const message of ['the selected environment is unavailable', 'the environment is not there or is being removed']) {
      const error = new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', title: 'Refused',
        detail: diagnostic, status: 400, request_id: '', errors: [{ path: 'environment.use', message }] }, null);
      expect(error.faults()).toEqual([{ path: 'environment.use', message: t('errors.environment.selected_unavailable') }]);
    }
  });

  it(`gives workspace attachment refusals a catalogued cause and recovery in ${locale}`, async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const failures = {
      workspace_file_path: 'ui.error.workspaceFilePath',
      workspace_file_name: 'ui.error.workspaceFileName',
      workspace_file_store: 'ui.error.workspaceFileStore',
      workspace_file_away: 'ui.error.workspaceFileAway',
      workspace_file_environment: 'ui.error.workspaceFileEnvironment',
      workspace_file_read: 'ui.error.workspaceFileRead',
      workspace_file_too_large: 'ui.error.workspaceFileTooLarge',
      workspace_file_changed: 'ui.error.workspaceFileChanged',
      workspace_file_destination_changed: 'ui.error.workspaceFileDestinationChanged',
    } as const;
    for (const [code, key] of Object.entries(failures)) {
      const message = errorText({ code, message: diagnostic }, t, { locale }).message;
      expect(message).toBe(t(key));
      expect(message).not.toBe(t('ui.error.operationFailed'));
      expect(hasErrorCode(message)).toBe(false);
    }
  });

  it(`maps every real API code to its own sentence and act in ${locale}`, async () => {
    await setLocale(locale);
    expect(codes.length).toBeGreaterThan(100);
    const t = i18n.getFixedT(locale);
    for (const code of codes) {
      const expected = sentence(locale === 'en' ? english : chinese, code);
      const error = new ApiError({ type: `orch:error:${code}`, code: code as ErrorCode,
        title: diagnostic, detail: diagnostic, status: 503, request_id: 'keep-for-logs' }, null);
      expect(apiErrorSentence(error.code, error.status, t, locale)).toBe(expected);
      expect(errorText(error, t, { locale }).message).toBe(expected);
      expect(error.message).toBe(expected);
      expect(hasErrorCode(expected)).toBe(false);
      expect(error.problem.detail).toBe(diagnostic);
      expect(error.requestId).toBe('keep-for-logs');
    }
  });

  it(`withholds embedded native codes and columns while preserving a useful field reason in ${locale}`, async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    for (const code of ['harness_unavailable', 'store_unavailable', 'conflict.stale_preflight', 'model_inputs_segment_id_fkey', 'threads_starting_step_check', 'conflict', 'auth', 'request']) {
      const message = errorText({ code, message: `This failed: ${diagnostic}` }, t, { locale }).message;
      expect(hasErrorCode(message)).toBe(false);
      expect(message).not.toContain(code);
    }
    expect(errorText({code:'conflict.stale_preflight',message:"the file job's environment changed"},t,{locale}).message)
      .toBe(t('errors.conflict.stale_revision'));
    const error = new ApiError({ type: 'orch:error:request.value_refused', code: 'request.value_refused', title: 'Refused', detail: diagnostic,
      status: 422, request_id: '', errors: [{ path: 'home_node_id', message: diagnostic }, { path: 'name', message: 'Choose a different name.' }] }, null);
    expect(error.faults()[0]).toEqual({ path: 'home_node_id', message: t('errors.request.invalid') });
    expect(error.faults()[1]).toEqual({ path: 'name', message: 'Choose a different name.' });
  });
}
