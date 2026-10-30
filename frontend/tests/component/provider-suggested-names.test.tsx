/** P6: accepted vendor names survive in saved rows, including a name identical to the routing name. */
import { cleanup, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ProviderModels } from '@/features/resources/providers/Models';
import type { ProviderModel } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
const models:ProviderModel[]=[
  { ...MODEL_ESTIMATE,id:'large',model_name:'Large example model',display_name:'Large example model',enabled:true,source:'suggested',groups:[],parameters:{}},
  { ...MODEL_ESTIMATE,id:'small',model_name:'mock-small',display_name:'Small example model',enabled:true,source:'suggested',groups:[],parameters:{}},
];
afterEach(async()=>{cleanup();document.documentElement.removeAttribute('data-theme');await setLocale('en');});
for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)
  it(`${locale}/${theme} shows both saved names and says an identical routing name only once`,async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;
    show(<ProviderModels provider={planProvider({models})} manage={false}/>);
    const rows=within(screen.getByRole('table',{name:i18n.t('providers.models.title')}));
    for(const model of models)expect(rows.getAllByText(model.display_name,{exact:true})).toHaveLength(1);
    expect(rows.queryByText(i18n.t('composer.model.displayUnknown'))).not.toBeInTheDocument();
  });
