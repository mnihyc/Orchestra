/** The shared field form keeps administrative credentials write-only and records removal explicitly. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach,expect,it,vi } from 'vitest';
import type { Field, SourceChoiceSet } from '@/generated/api-types';
import { SourceFields,emptySourceDraft,type SourceDraft } from '@/features/resources/providers/SourceFields';
import { sourceChoiceSet,localizedSourceFields } from '@/features/resources/providers/sourceOptions';
import { memberState } from '@/features/resources/providers/details';
import { i18n,setLocale } from '@/i18n';
import { Button } from '@/ui/Button';
import { SECRET_DOTS } from '@/ui/SecretField';
import { show } from './support';
const submitted=vi.fn<(value:SourceChoiceSet)=>void>();
const fields:Field[]=[{name:'admin_key',label:'Admin key',type:'secret',required:false,help:null,choices:null,default:null,max_bytes:null}];
const descriptor={facts:{admin_credential:{labels:{en:'Admin key','zh-CN':'管理密钥'},reports:['spend']}}};
function Form({saved=false}:{saved?:boolean}){
  const [draft,setDraft]=useState<SourceDraft>(emptySourceDraft());
  const local=localizedSourceFields(descriptor,fields,i18n.resolvedLanguage ?? i18n.language);
  return <><SourceFields element="facts" value={draft} onChange={setDraft} programs={[]} defaultTitle="Fixture source" defaultSummary="" defaultFields={local} httpAllowed canSendCredential savedAddress={null} authorizationSet={false} secretFieldsSet={saved?['admin_key']:[]}/><Button onClick={()=>submitted(sourceChoiceSet(draft,local,[]))}>{i18n.t('providerSources.save')}</Button></>;
}
beforeEach(()=>vi.clearAllMocks());
for(const locale of ['en','zh-CN'] as const){
  it(`writes a default source key once and keeps a saved value, shown as dots that are set, in ${locale}`,async()=>{
    await setLocale(locale);
    try{
      show(<Form saved/>);
      const label=`${locale==='en'?'Admin key':'管理密钥'} ${i18n.t('ui.field.optional')}`;
      // A saved key is never an empty box: it is dots, marked Set, with Replace.
      expect(screen.getByLabelText(label,{exact:true})).toHaveValue(SECRET_DOTS);
      expect(screen.getByText(i18n.t('settingsEditors.secret.set'))).toBeVisible();
      await userEvent.click(screen.getByRole('button',{name:i18n.t('providerSources.save')}));
      expect(submitted).toHaveBeenLastCalledWith({by:'default'});
      await userEvent.click(screen.getByRole('button',{name:i18n.t('settingsEditors.secret.replace')}));
      const input=screen.getByLabelText(label,{exact:true});
      expect(input).toHaveValue('');expect(input).toHaveAttribute('type','password');
      await userEvent.type(input,'new-admin-fixture');
      await userEvent.click(screen.getByRole('button',{name:i18n.t('providerSources.save')}));
      expect(submitted).toHaveBeenLastCalledWith({by:'default',secrets:{admin_key:'new-admin-fixture'}});
      expect(screen.queryByText('new-admin-fixture')).not.toBeInTheDocument();
    }finally{await setLocale('en');}
  });
  it(`removes a saved source key only by an explicit null write in ${locale}`,async()=>{
    await setLocale(locale);
    try{
      show(<Form saved/>);
      await userEvent.click(screen.getByRole('button',{name:i18n.t('settingsEditors.secret.remove')}));
      expect(screen.getByText(i18n.t('settingsEditors.secret.removed'))).toBeVisible();
      await userEvent.click(screen.getByRole('button',{name:i18n.t('providerSources.save')}));
      expect(submitted).toHaveBeenLastCalledWith({by:'default',secrets:{admin_key:null}});
    }finally{await setLocale('en');}
  });
}
it('keeps a reason-bearing unsupported state out of loading and amount states',()=>{
  expect(memberState({unsupported_because:{reason:'needs_admin_key'}})).toBe('unsupported');
  expect(memberState('not_read_yet')).toBe('not_read_yet');expect(memberState('told')).toBe('told');
});
