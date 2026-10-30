/** Scope settings use served native choices, retaining unsupported stored values until a person acts. */
import {screen,waitFor} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {expect,it,vi} from 'vitest';
import {putSetting} from '@/api/settings';
import type * as SettingsApi from '@/api/settings';
import {SettingRow} from '@/features/settings/SettingRow';
import type {KeyDef} from '@/features/settings/settingsModel';
import type {ResolvedSetting} from '@/generated/api-types';
import {i18n,setLocale} from '@/i18n';
import {show} from './support';

vi.mock('@/features/help/SettingHint',()=>({SettingHint:()=>null}));
vi.mock('@/api/settings',async(original)=>({...await original<typeof SettingsApi>(),putSetting:vi.fn(),clearSetting:vi.fn()}));

const target={kind:'node',id:'web'} as const;
const place={name:'Web',kind:'project',owner:null,access:{permissions:['project.manage','settings.manage']}};
const source={scope:'project',node:{id:'web',kind:'project',name:'Web',redacted:false}} as const;

it.each([
  {key:'approval.mode',stored:'auto',all:['ask','on_request','auto','bypass'],offered:'on_request',nameKey:'choiceCatalog.approvalMode.on_request.name',missingKey:'choiceCatalog.approvalMode.auto.name'},
  {key:'sandbox.mode',stored:'open',all:['read_only','workspace','open'],offered:'read_only',nameKey:'choiceCatalog.sandboxLevel.read_only.name',missingKey:'choiceCatalog.sandboxLevel.open.name'},
] as const)('offers only reported $key modes and writes a supported choice, in both catalogues',async(example)=>{
  try {
    for (const language of ['en','zh-CN'] as const) {
      await setLocale(language);
      const user=userEvent.setup();
      const def:KeyDef={key:example.key,title:example.key,description:'Native mode',group:'confinement',class:'parameter',scopes:['org','project','topic','thread'],merge:'replace',default:example.offered,example:example.offered,schema:{type:'string',enum:example.all},text_key:null,keywords:[]};
      const row:ResolvedSetting={key:example.key,value:example.stored,own:{value:example.stored,revision:3,locked:false},source,locked_by:null,writable:true,parts:[],available_values:[example.offered]};
      vi.mocked(putSetting).mockReset();
      vi.mocked(putSetting).mockResolvedValue({...row,value:example.offered,own:{value:example.offered,revision:4,locked:false}});
      const view=show(<ul><SettingRow def={def} setting={row} target={target} place={place} /></ul>);
      expect(putSetting).not.toHaveBeenCalled();
      await user.click(screen.getByRole('combobox',{name:example.key}));
      const choices=screen.getAllByRole('option');
      expect(choices).toHaveLength(1);
      const choice=choices[0];
      if (choice===undefined) throw new Error('the reported mode was not offered');
      expect(choice).toHaveAccessibleName(i18n.t(example.nameKey));
      expect(screen.queryByRole('option',{name:i18n.t(example.missingKey)})).not.toBeInTheDocument();
      await user.click(choice);
      await waitFor(()=>expect(putSetting).toHaveBeenCalledExactlyOnceWith(target,example.key,{value:example.offered,expectedRevision:3}));
      view.unmount();
    }
  } finally {await setLocale('en');}
});

it('does not offer Auto in the policy set while preserving the stored policy until it is edited',async()=>{
  const user=userEvent.setup();
  const def:KeyDef={key:'approval.allowed_modes',title:'Permitted modes',description:'Native modes the place permits',group:'confinement',class:'policy',scopes:['org','project','topic'],merge:'intersect',default:['ask','on_request','auto','bypass'],example:['on_request'],schema:{type:'array',uniqueItems:true,items:{type:'string',enum:['ask','on_request','auto','bypass']}},text_key:null,keywords:[]};
  const row:ResolvedSetting={key:def.key,value:['on_request','auto'],own:{value:['on_request','auto'],revision:3,locked:false},source,locked_by:null,writable:true,parts:[],available_values:['on_request']};
  vi.mocked(putSetting).mockReset();
  show(<ul><SettingRow def={def} setting={row} target={target} place={place} /></ul>);
  expect(putSetting).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:i18n.t('settings.compact.editValue',{name:def.title})}));
  expect(screen.getByRole('checkbox',{name:'On request'})).toBeChecked();
  expect(screen.queryByRole('checkbox',{name:'Auto'})).not.toBeInTheDocument();
  expect(putSetting).not.toHaveBeenCalled();
});
