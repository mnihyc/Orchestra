/** An unreadable provider stays deletable through the ordinary authority-checked route. */
import { screen,waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach,describe,expect,it,vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { deleteProvider } from '@/api/resources';
import { ApiError } from '@/api/client';
import { ProviderReadFailure } from '@/features/resources/providers/ReadFailure';
import { i18n,setLocale } from '@/i18n';
import { show } from './support';

const navigate=vi.hoisted(()=>vi.fn());
vi.mock('@tanstack/react-router',()=>({useNavigate:()=>navigate}));
vi.mock('@/api/resources',async(load)=>({...await load<typeof Resources>(),deleteProvider:vi.fn()}));
afterEach(async()=>{vi.clearAllMocks();document.documentElement.removeAttribute('data-theme');await setLocale('en');});
for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'])describe(`${locale}/${theme}`,()=>{
  it('keeps Retry and Delete available, confirms once and navigates only after acknowledgement',async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale);const retry=vi.fn();const user=userEvent.setup();
    const error=new ApiError({type:'orch:error:internal',status:500,code:'internal',title:'Unreadable provider',detail:'Facts could not be read',request_id:'read'},null);
    vi.mocked(deleteProvider).mockResolvedValue(undefined);
    show(<ProviderReadFailure id="broken" error={error} retrying={false} onRetry={retry}/>);
    expect(screen.queryByRole('button',{name:t('resourceCreation.providers')})).not.toBeInTheDocument();
    await user.click(screen.getByRole('button',{name:t('common.tryAgain')}));expect(retry).toHaveBeenCalledOnce();
    await user.click(screen.getByRole('button',{name:t('resources.act.delete')}));
    expect(screen.getByRole('alertdialog',{name:t('providerRecovery.deleteTitle')})).toBeVisible();
    expect(deleteProvider).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button',{name:t('providerRecovery.delete')}));
    await waitFor(()=>expect(navigate).toHaveBeenCalledExactlyOnceWith({to:'/resources/providers'}));
    expect(deleteProvider).toHaveBeenCalledExactlyOnceWith('broken');
  });
});
