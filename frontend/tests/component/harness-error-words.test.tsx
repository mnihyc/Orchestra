/** Harness facts get specific recovery copy and retain the program's escaped words below it. */
import { screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { ApiError } from '@/api/client';
import { FailureAlert } from '@/features/timeline/FailureAlert';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

const before=useAppearance.getState();
afterEach(async()=>{useAppearance.setState(before);await setLocale('en');});
const codes=['session_lost','harness_error','upstream_retrying','permission_denied','mcp_server_failed','fast_mode_paused'] as const;
for(const locale of ['en','zh-CN'] as const) for(const code of codes) it(`shows its own cause/recovery and native words for ${code} (${locale})`,async()=>{
  await setLocale(locale);useAppearance.setState({locale});
  const diagnostic=`Native ${code}: <script>not markup</script> · actual supplied detail`;
  const view=show(<FailureAlert error={{code,message:diagnostic,retryable:false}} attempts={2} />);
  const primary=screen.getByRole('alert');
  expect(primary).toHaveTextContent(i18n.t(`harnessFailures.${code}`));
  expect(primary).not.toHaveTextContent(i18n.t('ui.error.operationFailed'));
  expect(primary).not.toHaveTextContent(diagnostic);
  const note=screen.getByRole('note',{name:i18n.t('harnessFailures.programMessage')});
  expect(note).toHaveTextContent(diagnostic);
  expect(note.querySelector('script')).toBeNull();
  expect(primary.compareDocumentPosition(note)&Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
  expect(within(primary).queryByText(diagnostic)).toBeNull();
  view.rerender(<FailureAlert error={new ApiError({type:'orch:error:internal',status:500,code:'internal',title:'Internal',detail:diagnostic,request_id:'request'},null)} />);
  expect(screen.queryByRole('note')).toBeNull();
});

// A turn the agent program failed with the code internal is the program's failure, not the server's: its own words stand
// under the alert as for every other failure of a program (an answer of Orch's API that says internal stays the server's).
for(const locale of ['en','zh-CN'] as const) it(`reads a turn the program failed as internal as the program's failure, with its own words (${locale})`,async()=>{
  await setLocale(locale);useAppearance.setState({locale});
  show(<FailureAlert error={{code:'internal',message:'Writing footer.tsx failed',said:'disk quota exceeded',retryable:false,incomplete:false,resets_at:null,window:null}} />);
  const primary=screen.getByRole('alert');
  expect(primary).toHaveTextContent(i18n.t('harnessFailures.harness_error'));
  expect(primary).not.toHaveTextContent(i18n.t('errors.internal'));
  expect(screen.getByRole('note',{name:i18n.t('harnessFailures.programMessage')})).toHaveTextContent('Writing footer.tsx failed');
});
