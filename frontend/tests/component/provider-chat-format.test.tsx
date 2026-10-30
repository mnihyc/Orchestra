/** The compact provider form names the agents declared compatible with each format. */
import {screen} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {useState} from 'react';
import {afterEach,describe,expect,it,vi} from 'vitest';
import {EndpointsEditor,type EndpointDraft} from '@/features/resources/providers/Endpoints';
import type {ProviderSetupProtocol} from '@/generated/api-types';
import {i18n,setLocale} from '@/i18n';
import {useAppearance} from '@/lib/appearance';
import {show} from './support';

vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
const local={id:'local-agent',name:'GPU assistant',address:'Acme',harness_kind:'local',harness:{kind:'local',name:'GPU assistant'},machine:null,disabled:false};
const codex={...local,id:'codex',name:'Codex',harness_kind:'codex',harness:{kind:'codex',name:'Codex'}};
const protocols:ProviderSetupProtocol[]=[
  {name:'openai-chat',title:'OpenAI Chat Completions',agents:[local,codex],endpoint_options:[]},
  {name:'openai-responses',title:'OpenAI Responses',agents:[codex],endpoint_options:[]},
  {name:'anthropic-messages',title:'Anthropic Messages',agents:[],endpoint_options:[]},
];
function Form(){
  const [rows,setRows]=useState<EndpointDraft[]>([{id:'endpoint',protocol:'openai-chat',baseUrl:'https://gpu.example/v1',options:{}}]);
  return <EndpointsEditor compact value={rows} onChange={setRows} protocols={protocols} httpAllowed={false}/>;
}
afterEach(async()=>{useAppearance.getState().set({theme:'system',locale:'en'});await setLocale('en');});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)describe(`${locale}/${theme}`,()=>{
  it('names the available agent for the chosen format in the actual compact endpoint control',async()=>{
    await setLocale(locale);useAppearance.getState().set({locale,theme});
    const t=i18n.getFixedT(locale);const user=userEvent.setup();
    show(<Form/>);
    expect(screen.getAllByText(t('resources.endpoint.manyAgents',{agents:`${local.name}, ${codex.name}`}))).toHaveLength(1);
    expect(screen.queryByText(/Codex\s+(needs|需要)/u)).not.toBeInTheDocument();
    expect(screen.queryByText(t('resources.endpoint.noAgent'))).not.toBeInTheDocument();
    await user.click(screen.getByRole('combobox',{name:t('resources.endpoint.protocol')}));
    await user.click(screen.getByRole('option',{name:'OpenAI Responses'}));
    expect(screen.getAllByText(t('resources.endpoint.oneAgent',{agents:codex.name}))).toHaveLength(1);
    expect(screen.queryByText(/Codex\s+(needs|需要)/u)).not.toBeInTheDocument();
    expect(screen.queryByText(t('resources.endpoint.manyAgents',{agents:`${local.name}, ${codex.name}`}))).not.toBeInTheDocument();
    await user.click(screen.getByRole('combobox',{name:t('resources.endpoint.protocol')}));
    await user.click(screen.getByRole('option',{name:'Anthropic Messages'}));
    expect(screen.getAllByText(t('resources.endpoint.noAgent'))).toHaveLength(1);
  });
});
