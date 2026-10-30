/** A failed stored-state read is a recoverable error, not an endless loading state. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect,it,vi } from 'vitest';
import { HostRuntimes } from '@/features/resources/hosts/Runtimes';
import type { Host,Runtime } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';
const read=vi.hoisted(()=>vi.fn<()=>Promise<Runtime[]>>());
vi.mock('@/api/resources',()=>({hostRuntimesQuery:()=>({queryKey:['host-runtimes','host'],queryFn:read}),removeHostRuntime:vi.fn(),stopHostRuntime:vi.fn()}));
vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
it('names the failed read and offers Retry, then shows the recovered stored state',async()=>{
  read.mockRejectedValueOnce(new Error('The stored runtime report could not be read.')).mockResolvedValueOnce([]);
  const user=userEvent.setup();show(<HostRuntimes host={{id:'host'} as Host}/>);
  expect(await screen.findByText(i18n.t('hosts.runtimes.title'))).toBeVisible();
  expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('errors.page'));
  await user.click(screen.getByRole('button',{name:i18n.t('common.tryAgain')}));
  expect(await screen.findByText('Nothing runs on it')).toBeVisible();expect(read).toHaveBeenCalledTimes(2);
});
