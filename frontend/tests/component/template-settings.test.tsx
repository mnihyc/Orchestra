/** Template settings are searchable rows with honest sources, menu-only defaults and retained complex drafts. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { checkTemplate } from '@/api/resources';
import { TemplateEditor } from '@/features/resources/templates/TemplateEditor';
import { SchemaForm } from '@/features/resources/templates/SchemaSettings';
import type { Schema } from '@/features/resources/templates/schema';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

vi.mock('@/api/resources',async(original)=>({...await original<object>(),checkTemplate:vi.fn()}));
vi.mock('@/ui/CodeView',()=>({CodeView:({text,label,onChange}:{text:string;label:string;onChange?:(text:string)=>void})=><textarea aria-label={label} value={text} onChange={(event)=>onChange?.(event.target.value)} />}));
const schema:Schema={type:'object',properties:{
  image:{type:'string',title:'Image reference',description:'An image supplied by this driver.',default:null},
  resources:{type:'object',title:'Limits',properties:{cores:{type:'integer',title:'CPU cores',description:'Raise for parallel work.',default:4,minimum:1,maximum:16,'x-unit':'cores'},memory_mb:{type:'integer',title:'Memory',description:'Memory in MiB.',default:1024}}},
  env:{type:'object',title:'Variables',additionalProperties:{type:'string'},default:{}},
  future_choice:{type:'string',title:'Driver choice',description:'A choice shipped with a newer driver.',enum:['new-value','other-value'],default:'new-value'},
}};
const beforeTheme=useAppearance.getState();
const dark=document.documentElement.classList.contains('dark');
beforeEach(async()=>{await setLocale('en');vi.clearAllMocks();vi.mocked(checkTemplate).mockResolvedValue({valid:true,errors:[]});});
afterEach(async()=>{useAppearance.setState(beforeTheme);document.documentElement.classList.toggle('dark',dark);await setLocale('en');});

for(const locale of ['en','zh-CN'] as const) for(const theme of ['light','dark'] as const) {
  it(`uses shared rows and a gear-only default/reset, keeping source and unknown siblings (${locale}/${theme})`,async()=>{
    await setLocale(locale);useAppearance.setState({theme,resolvedTheme:theme});document.documentElement.classList.toggle('dark',theme==='dark');
    const changed=vi.fn();const user=userEvent.setup();
    function Form(){const [value,setValue]=useState<Record<string,unknown>>({image:'node:22-bookworm',resources:{cores:8,future_limit:9},extension:{keep:true}});return <SchemaForm schema={schema} spec={value} driver="sample" onChange={(next)=>{changed(next);setValue(next);}} />;}
    const view=show(<Form/>);
    const cpu=screen.getByRole('spinbutton',{name:'CPU cores'});
    const row=cpu.closest<HTMLElement>('[data-setting-row]');if(row===null)throw new Error('the field has no shared settings row');
    expect(within(row).getByText(i18n.t('templateSettings.source.template'))).toBeVisible();
    expect(screen.queryByText('Default: 4 cores')).not.toBeInTheDocument();
    expect(screen.queryByRole('button',{name:/Reset.*default/})).not.toBeInTheDocument();
    await user.click(within(row).getByRole('button',{name:i18n.t('settings.editor.menuOf',{name:'CPU cores'})}));
    expect(screen.getByText(i18n.t('templates.editor.default',{value:'4 cores'}))).toBeVisible();
    await user.click(screen.getByRole('menuitem',{name:i18n.t('templates.editor.reset')}));
    expect(cpu).toHaveValue(4);
    expect(within(row).getByText(i18n.t('templateSettings.source.driver'))).toBeVisible();
    expect(changed).toHaveBeenLastCalledWith({image:'node:22-bookworm',resources:{future_limit:9},extension:{keep:true}});
    expect(view.container.querySelectorAll('[data-setting-row]').length).toBeGreaterThan(3);
  });
}

it.each(['en','zh-CN'] as const)('filters localized fields and descriptions without losing edited values (%s)',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();const changed=vi.fn();
  function Form(){const [value,setValue]=useState<Record<string,unknown>>({image:'node:22-bookworm'});return <SchemaForm schema={schema} spec={value} driver="docker" onChange={(next)=>{changed(next);setValue(next);}}/>;}
  show(<Form/>);
  const search=screen.getByRole('searchbox',{name:i18n.t('templateSettings.search')});
  const memoryName=locale==='en'?'Memory':'内存';
  await user.type(search,memoryName);
  expect(screen.queryByRole('textbox',{name:locale==='en'?'Image reference':'镜像引用'})).not.toBeInTheDocument();
  const memory=screen.getByRole('spinbutton',{name:memoryName});fireEvent.change(memory,{target:{value:'3072'}});
  await user.clear(search);expect(memory).toHaveValue(3072);
  await user.type(search,'nothing-matches-this');
  expect(screen.getByText(i18n.t('templateSettings.noMatch'))).toBeVisible();
  await user.clear(search);expect(screen.getByRole('spinbutton',{name:memoryName})).toHaveValue(3072);
  expect(changed).toHaveBeenLastCalledWith({image:'node:22-bookworm',resources:{memory_mb:3072}});
});

it('retains an invalid auxiliary draft when search hides its row, without more validation requests',async()=>{
  const user=userEvent.setup();const validity=vi.fn();
  function Form(){const [text,setText]=useState(JSON.stringify({image:'node:22-bookworm',extension:{keep:true}}));return <TemplateEditor schema={schema} driver="sample" kind="environment" text={text} onChange={setText} onValidityChange={validity}/>;}
  show(<Form/>);await waitFor(()=>expect(checkTemplate).toHaveBeenCalledOnce());
  await user.click(screen.getByRole('button',{name:'Edit Other settings (JSON)'}));
  const raw=screen.getByRole('textbox',{name:'Other settings (JSON)'});fireEvent.change(raw,{target:{value:'{'}});
  await waitFor(()=>expect(validity).toHaveBeenLastCalledWith(false));
  const search=screen.getByRole('searchbox',{name:'Search template settings'});await user.type(search,'CPU');
  expect(screen.queryByRole('textbox',{name:'Other settings (JSON)'})).not.toBeInTheDocument();
  await user.clear(search);
  expect(screen.getByRole('textbox',{name:'Other settings (JSON)'})).toHaveValue('{');
  expect(screen.getByRole('switch',{name:'JSON'})).toBeDisabled();
  expect(checkTemplate).toHaveBeenCalledOnce();
});

it('a reader gets values and source chips, with no disabled form or editable JSON',()=>{
  const changed=vi.fn();const view=show(<SchemaForm schema={schema} spec={{image:'node:22-bookworm',resources:{cores:6},future_choice:'other-value'}} driver="sample" readOnly onChange={changed}/>);
  expect(screen.getByText('node:22-bookworm')).toBeVisible();
  expect(screen.getByText('6 cores')).toBeVisible();
  expect(screen.getByText('other-value')).toBeVisible();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument();
  expect(view.container.querySelectorAll('[data-setting-description]')).toHaveLength(5);
  expect(view.container.querySelectorAll('[data-template-source]').length).toBeGreaterThan(1);
  expect(changed).not.toHaveBeenCalled();
});

it('opens a declared structured default without materializing it until the person edits it',async()=>{
  const user=userEvent.setup();const changed=vi.fn();
  const withDefaults:Schema={type:'object',properties:{env:{type:'object',title:'Variables',additionalProperties:{type:'string'},default:{MODE:'development'}}}};
  show(<SchemaForm schema={withDefaults} spec={{}} driver="sample" onChange={changed}/>);
  await user.click(screen.getByRole('button',{name:'Edit Variables'}));
  expect(screen.getByRole('textbox',{name:'Name'})).toHaveValue('MODE');
  expect(screen.getByRole('textbox',{name:'Value'})).toHaveValue('development');
  expect(changed).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole('textbox',{name:'Value'}),{target:{value:'production'}});
  expect(changed).toHaveBeenLastCalledWith({env:{MODE:'production'}});
});

it('allows a person to clear and replace the displayed numeric default without prefixing the old number',async()=>{
  const user=userEvent.setup();const changed=vi.fn();
  function Form(){const [value,setValue]=useState<Record<string,unknown>>({resources:{future_limit:9}});return <SchemaForm schema={schema} spec={value} driver="sample" onChange={(next)=>{changed(next);setValue(next);}}/>;}
  show(<Form/>);
  const cpu=screen.getByRole('spinbutton',{name:'CPU cores'});expect(cpu).toHaveValue(4);
  await user.clear(cpu);expect(cpu).toHaveValue(null);
  await user.type(cpu,'12');await user.tab();
  expect(cpu).toHaveValue(12);
  expect(changed).toHaveBeenLastCalledWith({resources:{cores:12,future_limit:9}});
});

it('keeps ordinary fields intact when editing the other-settings object',async()=>{
  const user=userEvent.setup();const changed=vi.fn();
  show(<SchemaForm schema={schema} spec={{image:'node:22-bookworm',extension:{keep:true}}} driver="sample" onChange={changed}/>);
  await user.click(screen.getByRole('button',{name:'Edit Other settings (JSON)'}));
  fireEvent.change(screen.getByRole('textbox',{name:'Other settings (JSON)'}),{target:{value:'{"image":"do-not-replace","extension":{"keep":true},"new_setting":7}'}});
  expect(changed).toHaveBeenLastCalledWith({image:'node:22-bookworm',extension:{keep:true},new_setting:7});
});

it('switches image sources without discarding variables or driver extensions',async()=>{
  const user=userEvent.setup();
  const sources:Schema={type:'object',properties:{image:{type:'string',title:'Image reference'},image_digest:{type:'string',title:'Pinned digest'},build:{type:'object',required:['dockerfile'],properties:{dockerfile:{type:'string',title:'Dockerfile','x-multiline':true}}},archive:{type:'object',required:['blob'],properties:{blob:{type:'string',title:'Archive hash'}}},env:{type:'object',title:'Variables',additionalProperties:{type:'string'}}}};
  function Form(){const [text,setText]=useState(JSON.stringify({image:'node:22-bookworm',image_digest:'old-digest',env:{BUILD_FLAGS:'a b'},extension:{keep:true}}));return <TemplateEditor schema={sources} text={text} driver="sample" kind="environment" onChange={setText}/>;}
  show(<Form/>);
  await user.click(screen.getByRole('combobox',{name:i18n.t('templates.editor.imageSource')}));
  await user.click(screen.getByRole('option',{name:i18n.t('templates.sourceChoices.build.name')}));
  expect(screen.queryByRole('textbox',{name:'Image reference'})).not.toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:'Edit Dockerfile'}));
  fireEvent.change(screen.getByRole('textbox',{name:'Dockerfile'}),{target:{value:'FROM node:22-bookworm'}});
  await user.click(screen.getByRole('switch',{name:'JSON'}));
  expect(JSON.parse(screen.getByRole<HTMLTextAreaElement>('textbox',{name:'Configuration (JSON)'}).value)).toEqual({build:{dockerfile:'FROM node:22-bookworm'},env:{BUILD_FLAGS:'a b'},extension:{keep:true}});
  await user.click(screen.getByRole('switch',{name:'JSON'}));
  await user.click(screen.getByRole('button',{name:'Edit Dockerfile'}));
  expect(screen.getByRole('textbox',{name:'Dockerfile'})).toHaveValue('FROM node:22-bookworm');
});

it('the compact page keeps an unfinished JSON row reachable and blocks search from remounting it',async()=>{
  const user=userEvent.setup();const validity=vi.fn();
  function Form(){const [text,setText]=useState(JSON.stringify({image:'node:22-bookworm',extension:{keep:true}}));return <TemplateEditor compact schema={schema} driver="sample" kind="environment" text={text} onChange={setText} onValidityChange={validity}/>;}
  show(<Form/>);await waitFor(()=>expect(checkTemplate).toHaveBeenCalledOnce());
  await user.click(screen.getByRole('button',{name:'Edit Other settings (JSON)'}));
  const raw=screen.getByRole('textbox',{name:'Other settings (JSON)'});
  fireEvent.change(raw,{target:{value:'{'}});
  await waitFor(()=>expect(validity).toHaveBeenLastCalledWith(false));
  expect(screen.getByRole('searchbox',{name:'Search template settings'})).toBeDisabled();
  raw.focus();await user.keyboard('{Escape}');
  await user.click(screen.getByRole('button',{name:'Edit Other settings (JSON)'}));
  expect(screen.getByRole('textbox',{name:'Other settings (JSON)'})).toHaveValue('{');
  fireEvent.change(screen.getByRole('textbox',{name:'Other settings (JSON)'}),{target:{value:'{"extension":{"keep":true}}'}});
  await waitFor(()=>expect(validity).toHaveBeenLastCalledWith(true));
  expect(screen.getByRole('searchbox',{name:'Search template settings'})).toBeEnabled();
  expect(checkTemplate).toHaveBeenCalledOnce();
});
