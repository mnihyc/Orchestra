/** Details source choices keep address authorization write-only and require consent naming the exact recipient of a provider key. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { emptySourceDraft, SourceFields, sourceFault, type SourceDraft, type SourceFieldsProps } from '@/features/resources/providers/SourceFields';
import { providerSourcesBody, SourcesForm, sourceDraft, type SourceOffers, type SourcesFormProps } from '@/features/resources/providers/Sources';
import type { Field, Provider } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { i18n, setLocale } from '@/i18n';
import { choiceCopy } from '@/i18n/choices';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@/features/help/open', () => ({useHelpOpener:() => vi.fn()}));

const save = vi.fn<(_draft:SourceDraft) => void>();
const base: Omit<SourceFieldsProps,'value' | 'onChange'> = { element:'facts',programs:[],defaultTitle:'Default reader',defaultSummary:'Uses the registered default.',httpAllowed:true,canSendCredential:true,savedAddress:null,authorizationSet:false,secretFieldsSet:[] };
function Form({initial = {...emptySourceDraft(),by:'address' as const,url:'https://reports.example/details'},...props}:Partial<Omit<SourceFieldsProps,'value' | 'onChange'>> & {initial?:SourceDraft}) {
  const [value,setValue] = useState(initial);
  const context = {...base,...props};
  return <><SourceFields {...context} value={value} onChange={setValue} /><Button disabled={sourceFault(value,context) !== null} onClick={() => save(value)}>Save sources</Button></>;
}
beforeEach(() => { vi.clearAllMocks(); });

it.each(['en','zh-CN'] as const)('places a server refusal on its source address without dropping its words or draft in %s',async(locale)=>{
  await setLocale(locale);
  try {
    const message=locale==='en'?'This machine cannot reach that reporting address.':'此机器无法访问该报告地址。';
    show(<Form serverErrors={{'address.url':message}}/>);
    const address=screen.getByRole('textbox',{name:i18n.t('providerSources.url')});
    expect(address).toHaveAttribute('aria-invalid','true');expect(address).toHaveAccessibleDescription(message);
    expect(address).toHaveValue('https://reports.example/details');
    expect(screen.getAllByText(message)).toHaveLength(1);
  } finally {await setLocale('en');}
});

it('names the host in a Cancel-first key consent and clears that consent when the address changes', async () => {
  show(<Form />);
  const consent=screen.getByRole('checkbox',{name:'Send this provider’s key to this address'});
  await userEvent.click(consent);
  const confirm = screen.getByRole('alertdialog',{name:'Send the key to reports.example?'});
  expect(confirm).toHaveTextContent('reports.example');
  // The modal hides the background from accessibility queries; its checkbox still has no consent.
  expect(consent).toBeInTheDocument();
  expect(consent).not.toBeChecked();
  await waitFor(() => expect(within(confirm).getByRole('button',{name:'Cancel'})).toHaveFocus());
  await userEvent.click(within(confirm).getByRole('button',{name:'Cancel'}));
  expect(screen.getByRole('checkbox',{name:'Send this provider’s key to this address'})).not.toBeChecked();
  await userEvent.click(screen.getByRole('checkbox',{name:'Send this provider’s key to this address'}));
  await userEvent.click(screen.getByRole('button',{name:'Send this key'}));
  expect(screen.getByRole('checkbox',{name:'Send this provider’s key to this address'})).toBeChecked();
  await userEvent.clear(screen.getByRole('textbox',{name:'Address'}));
  await userEvent.type(screen.getByRole('textbox',{name:'Address'}),'https://another.example/details');
  expect(screen.getByRole('checkbox',{name:'Send this provider’s key to this address'})).not.toBeChecked();
  await userEvent.click(screen.getByRole('button',{name:'Save sources'}));
  expect(save).toHaveBeenCalledWith(expect.objectContaining({url:'https://another.example/details',sendCredential:false}));
});

it('marks plain HTTP and refuses Save while its policy is off', () => {
  show(<Form initial={{...emptySourceDraft(),by:'address',url:'http://internal.example/details'}} httpAllowed={false} />);
  expect(screen.getByText('Unencrypted')).toBeVisible();
  expect(screen.getByText(i18n.t('providerSources.httpOff'))).toBeVisible();
  expect(screen.getByRole('button',{name:'Save sources'})).toBeDisabled();
  expect(save).not.toHaveBeenCalled();
});

it('keeps a saved authorization blank and requires a new value for a different address', async () => {
  show(<Form initial={{...emptySourceDraft(),by:'address',url:'https://reports.example/details',auth:'bearer'}} savedAddress={{url:'https://reports.example/details',auth:'bearer',header:''}} authorizationSet />);
  expect(screen.getByLabelText('Authorization value')).toHaveValue('');
  expect(screen.getByText('Set; replace')).toBeVisible();
  expect(screen.getByRole('button',{name:'Save sources'})).toBeEnabled();
  await userEvent.clear(screen.getByRole('textbox',{name:'Address'}));
  await userEvent.type(screen.getByRole('textbox',{name:'Address'}),'https://other.example/details');
  expect(screen.getByRole('button',{name:'Save sources'})).toBeDisabled();
  await userEvent.type(screen.getByLabelText('Authorization value'),'a-new-secret');
  expect(screen.getByLabelText('Authorization value')).toHaveAttribute('type','password');
  expect(screen.queryByText('a-new-secret')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button',{name:'Save sources'}));
  expect(save).toHaveBeenCalledWith(expect.objectContaining({authorization:'a-new-secret',url:'https://other.example/details'}));
});

it('never offers a sign-in to an address', () => {
  show(<Form canSendCredential={false} />);
  expect(screen.queryByRole('checkbox',{name:'Send this provider’s key to this address'})).not.toBeInTheDocument();
  expect(screen.getByText('A sign-in stays with its harness and is never sent to an address.')).toBeVisible();
});

it('offers Manual only for models and only programs already supplied at the keeper', async () => {
  show(<Form element="models" initial={emptySourceDraft()} programs={[{name:'acme',title:'Acme plan',summary:'The registered example.',reports:[],fields:[],command:['python','/opt/acme.py'],templateId:'provider-template'}]} />);
  await userEvent.click(screen.getByRole('combobox',{name:'Models from'}));
  expect(screen.queryByRole('option',{name:'Nothing'})).not.toBeInTheDocument();
  const manual = screen.getByRole('option',{name:choiceCopy(i18n.t,'sourceKind.manual').name});
  expect(manual).toBeVisible();
  expect(manual).toHaveAccessibleDescription(choiceCopy(i18n.t,'sourceKind.manual').description);
  await userEvent.click(screen.getByRole('option',{name:'Program'}));
  await userEvent.click(screen.getByRole('combobox',{name:'Registered program'}));
  await userEvent.click(screen.getByRole('option',{name:'Acme plan'}));
  expect(screen.getByText('python /opt/acme.py')).toBeVisible();
  expect(screen.getByText('Choosing this program uses the provider template that carries it.')).toBeVisible();
  expect(screen.queryByRole('textbox',{name:'Command'})).not.toBeInTheDocument();
});

const offers:SourceOffers={defaultTitle:'Default reader',defaultSummary:'Reads from the default source.',programs:[]};
function SavedSources({initial,onSave}:{initial:Provider;onSave:SourcesFormProps['onSave']}) {
  const [provider,setProvider]=useState(initial);
  return <SourcesForm provider={provider} facts={offers} models={offers} httpAllowed onSave={async (change,reviewed) => {const fresh=await onSave(change,reviewed);setProvider(fresh);return fresh;}} />;
}

it('writes the reviewed revision and only the changed source, then clears the successful authorization draft', async () => {
  const initial=planProvider({kind:'api',revision:7});
  const fresh=structuredClone(initial);fresh.revision=8;
  fresh.sources.facts={by:'address',name:null,address:{url:'https://reports.example/read',auth:'bearer',send_credential:false},config:{},secrets:[],descriptor:null,described_at:null,last_error:null};
  fresh.secrets_set=['source:facts:authorization'];
  const sent=vi.fn<SourcesFormProps['onSave']>().mockResolvedValue(fresh);
  show(<SavedSources initial={initial} onSave={sent} />);
  const facts=within(screen.getByRole('group',{name:'Details from'}));
  await userEvent.click(facts.getByRole('combobox',{name:'Details from'}));
  await userEvent.click(screen.getByRole('option',{name:'Address'}));
  await userEvent.type(facts.getByRole('textbox',{name:'Address'}),'https://reports.example/read');
  await userEvent.click(facts.getByRole('combobox',{name:'Authorization'}));
  await userEvent.click(screen.getByRole('option',{name:'Bearer token'}));
  await userEvent.type(facts.getByLabelText('Authorization value'),'typed-source-token');
  await userEvent.click(screen.getByRole('button',{name:'Save sources'}));
  await screen.findByText('Sources saved');
  const [change,reviewed]=sent.mock.calls[0]!;
  expect(providerSourcesBody(change,reviewed,offers,offers)).toEqual({expected_revision:7,facts:{by:'address',address:{url:'https://reports.example/read',auth:'bearer',send_credential:false},config:{},secrets:{},authorization:'typed-source-token'}});
  expect(change.models).toBeNull();
  expect(facts.getByLabelText('Authorization value')).toHaveValue('');
  expect(facts.getByText('Set; replace')).toBeVisible();
  expect(screen.queryByText('typed-source-token')).not.toBeInTheDocument();
});

it('keeps an authorization draft on a refused save and on a newer provider until the person chooses current values', async () => {
  const initial=planProvider({kind:'api'});
  initial.sources.facts={by:'address',name:null,address:{url:'https://reports.example/read',auth:'bearer',send_credential:false},config:{},secrets:[],descriptor:null,described_at:null,last_error:null};
  initial.secrets_set=['source:facts:authorization'];
  const refused=vi.fn<SourcesFormProps['onSave']>().mockRejectedValue(new Error('refused'));
  const props={facts:offers,models:offers,httpAllowed:true,onSave:refused};
  const view=show(<SourcesForm provider={initial} {...props} />);
  const facts=within(screen.getByRole('group',{name:'Details from'}));
  await userEvent.type(facts.getByLabelText('Authorization value'),'keep-this-draft');
  await userEvent.click(screen.getByRole('button',{name:'Save sources'}));
  await waitFor(() => expect(refused).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.getByRole('button',{name:'Save sources'})).toBeEnabled());
  expect(facts.getByLabelText('Authorization value')).toHaveValue('keep-this-draft');
  const newer=structuredClone(initial);newer.revision+=1;
  view.rerender(<SourcesForm provider={newer} {...props} />);
  expect(screen.getByText(/This provider changed while you were editing/)).toBeVisible();
  expect(screen.getByRole('button',{name:'Save sources'})).toBeDisabled();
  expect(facts.getByLabelText('Authorization value')).toHaveValue('keep-this-draft');
  await userEvent.click(screen.getByRole('button',{name:'Use current values'}));
  expect(facts.getByLabelText('Authorization value')).toHaveValue('');
  expect(refused).toHaveBeenCalledTimes(1);
});

it('never prefills a source secret from its declaration or public config, and requires it only when none was saved', async () => {
  const field:Field={name:'token',label:'Program token',type:'secret',required:true,default:'unsafe-declared-secret',choices:null,help:null,max_bytes:null};
  const provider=planProvider();
  const choice={...provider.sources.facts,by:'program' as const,name:'acme',config:{token:'unsafe-public-secret'},descriptor:{...provider.sources.facts.descriptor!,fields:[field]}};
  const draft=sourceDraft(choice);
  expect(draft.fields.token).toBe('');
  show(<Form initial={draft} programs={[{name:'acme',title:'Acme',summary:'The registered source.',reports:[],fields:[field],command:null,templateId:null}]} />);
  expect(screen.getByLabelText('Program token')).toHaveValue('');
  expect(screen.getByRole('button',{name:'Save sources'})).toBeDisabled();
  await userEvent.type(screen.getByLabelText('Program token'),'typed-program-token');
  expect(screen.getByRole('button',{name:'Save sources'})).toBeEnabled();
  expect(screen.queryByText('unsafe-declared-secret')).not.toBeInTheDocument();
  expect(screen.queryByText('unsafe-public-secret')).not.toBeInTheDocument();
});
