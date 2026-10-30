/** Ordinary screens name capabilities and choices, keep custom names, and never expose addresses as names. */
import { cleanup, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as AdminApi from '@/api/admin';
import { Capabilities } from '@/features/admin/Capabilities';
import { SettingReadout } from '@/features/settings/SettingReadout';
import { SettingEntry } from '@/features/help/SettingEntry';
import { localizedSetting } from '@/features/settings/settingText';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { capabilityDescription, capabilityName } from '@/features/resources/capabilityWords';
import type { CapabilityDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { UserChip } from '@/ui/UserChip';
import { show } from './support';

const reads=vi.hoisted(()=>{const rows:CapabilityDef[]=[];return {rows};});
vi.mock('@/api/admin',async(original)=>({...await original<typeof AdminApi>(),capabilitiesQuery:{queryKey:['person-capabilities'],queryFn:()=>Promise.resolve(reads.rows)}}));
const appearance=useAppearance.getState();
const previousTheme=document.documentElement.getAttribute('data-theme');
afterEach(async()=>{cleanup();useAppearance.setState(appearance);await setLocale('en');if(previousTheme===null)document.documentElement.removeAttribute('data-theme');else document.documentElement.dataset.theme=previousTheme;});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const){
  it(`names built-in and custom capabilities in the actual management table in ${locale}/${theme}`,async()=>{
    await setLocale(locale);useAppearance.setState({locale,theme});document.documentElement.dataset.theme=theme;
    reads.rows=[['docker_socket','docker_socket'],['host_network','Host network'],['kvm','KVM virtualisation'],['custom_key','Bench tool']].map(([key='',label='']):CapabilityDef=>({key,label,description:key==='custom_key'?'A custom lab tool.':i18n.getFixedT('en')(`capabilityWords.${key}.description`,{defaultValue:''}),builtin:key!=='custom_key',default_decision:'ask',default_approver_scope:'host',risk:'high',driver_patch:{docker:{}},revision:1}));
    const view=show(<Capabilities />);
    const table=await screen.findByRole('table',{name:i18n.t('admin.sections.capabilities.title')});
    for(const key of ['docker_socket','host_network','kvm'] as const){
      expect(within(table).getByText(i18n.t(`capabilityWords.${key}.name`))).toBeVisible();
      expect(within(table).getByText(i18n.t(`capabilityWords.${key}.description`))).toBeVisible();
      expect(table).not.toHaveTextContent(key);
    }
    expect(within(table).getByText('Bench tool')).toBeVisible();expect(table).not.toHaveTextContent('custom_key');
    expect(view.container).not.toHaveTextContent('orch-environment-');
    expect(capabilityName('kvm','Our emulator',i18n.t)).toBe('Our emulator');
    expect(capabilityDescription('kvm','Our lab instructions.',i18n.t)).toBe('Our lab instructions.');
    expect(capabilityName('lab','lab',i18n.t)).toBe('lab');
    expect(capabilityName('future_key','',i18n.t)).toBe(i18n.t('capabilityWords.unreported'));
  });

  it(`reads changed settings as named values and a person as their name in ${locale}/${theme}`,async()=>{
    await setLocale(locale);useAppearance.setState({locale,theme});document.documentElement.dataset.theme=theme;
    const view=show(<><SettingReadout settingKey="approval.allowed_modes" from={{value:['ask']}} value={['ask','on_request','auto','bypass']} /><SettingReadout settingKey="env.resources" value={{cpu_millis:2000,memory_swap_mb:8192,blkio_weight:250}} /><SettingReadout settingKey="mcp.enabled" value={{'00000000-0000-4000-8000-000000000123':true}} />
      <UserChip card={false} showUsername user={{id:'alice',kind:'human',username:'alice',display_name:'Alice',address:'acme.alice',avatar:{text:'A',color:2}}} /></>);
    expect(screen.getByText(i18n.t('settingRegistry.approval.allowed_modes.title'))).toBeVisible();
    for(const mode of ['ask','on_request','auto','bypass'] as const)expect(screen.getAllByText(i18n.t(`choiceCatalog.approvalMode.${mode}.name`)).length).toBeGreaterThan(0);
    expect(view.container).not.toHaveTextContent('on_request');expect(view.container).not.toHaveTextContent('approval.allowed_modes');expect(view.container).not.toHaveTextContent('["');
    expect(screen.getByText('Alice')).toBeVisible();expect(screen.getByText('@alice')).toBeVisible();expect(view.container).not.toHaveTextContent('acme.alice');
    expect(screen.getByText(i18n.t('trace.before'))).toBeVisible();expect(screen.getByText(i18n.t('trace.after'))).toBeVisible();
    for(const field of ['cpu_millis','memory_swap_mb','blkio_weight'] as const)expect(screen.getByText(i18n.t(`access.options.${field}`))).toBeVisible();
    expect(view.container).not.toHaveTextContent(/cpu_millis|memory_swap_mb|blkio_weight/);
    expect(screen.getByText(i18n.t('settings.compact.enabledCount',{count:1}))).toBeVisible();
    expect(view.container).not.toHaveTextContent('00000000-0000-4000-8000-000000000123');
  });
  it(`keeps a setting's allowed values, default and example readable in its help in ${locale}/${theme}`,async()=>{
    await setLocale(locale);useAppearance.setState({locale,theme});document.documentElement.dataset.theme=theme;
    const entry=SETTINGS_REGISTRY.find((def)=>def.key==='approval.allowed_modes');
    if(entry===undefined)throw new Error('The approval-mode policy must be in the shipped registry');
    const def=localizedSetting(entry,i18n);
    const view=show(<SettingEntry def={def} onBack={()=>undefined} />);
    expect(screen.getByRole('heading',{name:def.title})).toBeVisible();
    expect(screen.getByRole('button',{name:i18n.t('helpRelease.copyKey')})).toBeVisible();
    expect(screen.getByRole('heading',{name:i18n.t('helpRelease.shipped')})).toBeVisible();
    expect(screen.getByRole('heading',{name:i18n.t('helpRelease.example')})).toBeVisible();
    for(const mode of ['ask','on_request','auto','bypass'] as const)expect(screen.getAllByText(i18n.t(`choiceCatalog.approvalMode.${mode}.name`)).length).toBeGreaterThan(0);
    expect(view.container.querySelector('pre')).toBeNull();
    expect(view.container).not.toHaveTextContent(/approval\.allowed_modes|on_request|\[\s*"/);
  });

  it(`uses the composer's model name in a setting review in ${locale}/${theme}`,async()=>{
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme=theme;
    const view=show(<SettingReadout settingKey="agent.model" value="mock-large" />);
    expect(screen.getByText(i18n.t('composer.model.exampleLarge'))).toBeVisible();
    expect(view.container).not.toHaveTextContent(/mock-large|Mock Large/);
  });
}
