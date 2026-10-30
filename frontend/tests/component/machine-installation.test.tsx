/** An installation is observed data, including exact instance commands; placement never invents a service. */
import { screen } from '@testing-library/react';
import { expect,it } from 'vitest';
import { Installation,installationOf } from '@/features/resources/machines/Installation';
import type { MachineInstallation } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { show } from './support';
const connector:MachineInstallation={program:'orch-connector',executable:'/home/ada/.local/bin/orch-connector',data_dir:'/data/work',config_file:'/data/work/config.toml',system:false,user:'ada',service:'orch-connector-work.service',commands:{policy:'orch-connector --data-dir /data/work policy',roles:'orch-connector --data-dir /data/work roles',start:'orch-connector --data-dir /data/work service start',stop:'orch-connector --data-dir /data/work service stop',update:'orch-connector --data-dir /data/work update',uninstall:'orch-connector --data-dir /data/work uninstall'}};
for(const locale of ['en','zh-CN'] as const)it(`shows the actual installation and each lifecycle command once (${locale})`,async()=>{
  await setLocale(locale);
  try {
    show(<Installation facts={{}} installation={connector} version="1.2.3"/>);
    for(const value of ['ada','orch-connector-work.service','data/work','1.2.3'])expect(screen.getAllByText(value)).toHaveLength(1);
    expect(screen.queryByText(connector.executable,{exact:true})).not.toBeInTheDocument();
    for(const action of ['start','stop','update','uninstall'] as const){expect(connector.commands[action]).toBeTypeOf('string');expect(screen.getAllByText(connector.commands[action] ?? '')).toHaveLength(1);}
    expect(screen.getByText(i18n.t('machineInstallation.version'))).toBeVisible();
    const hint=screen.getByRole('button',{name:i18n.t('help.hint.label',{term:i18n.t('machineInstallation.help.name')})});
    expect(hint).toHaveAttribute('data-help-article','storage-cleanup');
    expect(hint).toHaveAttribute('data-help-heading','uninstall');
  }finally{await setLocale('en');}
});
it('keeps missing reports unknown and does not expose a guessed service command',()=>{
  expect(installationOf({placement:'managed'})).toBeNull();
  show(<Installation facts={{placement:'managed'}} version="1.2.3"/>);
  expect(screen.getByText(i18n.t('machineInstallation.notReported'))).toBeVisible();
  expect(screen.queryByText('systemctl start orch-node.service')).not.toBeInTheDocument();
});

it('offers the plan-based uninstall for an instance that runs in the foreground',()=>{
  show(<Installation facts={{}} installation={{...connector,service:null}} version="1.2.3"/>);
  expect(screen.getByText(connector.commands.uninstall ?? '')).toBeVisible();
});

it('keeps observed installation details but shows no uninstall command without ownership evidence',()=>{
  show(<Installation facts={{}} installation={{...connector,commands:{...connector.commands,uninstall:null}}} version="1.2.3"/>);
  expect(screen.getByText(i18n.t('machineInstallation.uninstallUnknown'))).toBeVisible();
  expect(screen.queryByText(connector.commands.uninstall ?? '')).not.toBeInTheDocument();
  expect(screen.getByText(connector.commands.start)).toBeVisible();
});
it('keeps a legacy service-removal string out of the uninstall control',()=>{
  const legacy={installation:{...connector,commands:{policy:connector.commands.policy,roles:connector.commands.roles,start:connector.commands.start,stop:connector.commands.stop,update:connector.commands.update,remove:'legacy service uninstall'}}};
  const parsed=installationOf(legacy);
  expect(parsed?.commands.uninstall).toBeNull();
  show(<Installation facts={legacy} version="1.2.3"/>);
  expect(screen.queryByText('legacy service uninstall')).not.toBeInTheDocument();
  expect(screen.getByText(i18n.t('machineInstallation.uninstallUnknown'))).toBeVisible();
});
