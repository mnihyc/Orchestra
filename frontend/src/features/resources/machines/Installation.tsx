/** Observed account, service and data directory, with exact local commands for this one installation. */
import { useHelpOpener } from '@/features/help/open';
import { useTranslation } from 'react-i18next';
import type { MachineInstallation } from '@/generated/api-types';
import { CommandBox,Facts } from '@/features/resources/Facts';
import { HelpHint } from '@/ui/HelpHint';
import { PathValue } from '@/features/resources/TechnicalValue';
const ACTIONS=['start','stop','update','uninstall'] as const;

function object(value:unknown):Record<string,unknown>|null {
  return value!==null && typeof value==='object' && !Array.isArray(value)?value as Record<string,unknown>:null;
}
function optional(value:unknown):string|null {return typeof value==='string'?value:null;}

/** Missing legacy installation reports remain unknown; machine placement never supplies a service or account. */
export function installationOf(facts:unknown):MachineInstallation|null {
  const source=object(object(facts)?.installation);
  const commands=object(source?.commands);
  if(source===null || commands===null || typeof source.program!=='string' || typeof source.executable!=='string'
    || typeof source.data_dir!=='string' || typeof source.config_file!=='string' || typeof source.system!=='boolean'
    || typeof commands.start!=='string' || typeof commands.stop!=='string' || typeof commands.update!=='string'
    || (commands.uninstall!==undefined && commands.uninstall!==null && typeof commands.uninstall!=='string') || typeof commands.policy!=='string') return null;
  return {program:source.program,executable:source.executable,data_dir:source.data_dir,config_file:source.config_file,system:source.system,
    user:optional(source.user),service:optional(source.service),commands:{start:commands.start,stop:commands.stop,update:commands.update,
      uninstall:optional(commands.uninstall),policy:commands.policy,roles:optional(commands.roles)}};
}

export function Installation({facts,version,installation:provided}:{facts:unknown;version:string;installation?:MachineInstallation|null}) {
  const {t}=useTranslation();
  const openHelp=useHelpOpener();
  const heading='uninstall';
  const installation=provided ?? installationOf(facts);
  return <section aria-labelledby="machine-installation" className="flex flex-col gap-3">
    <div className="flex items-center gap-1"><h2 id="machine-installation" className="font-medium">{t('machineInstallation.title')}</h2>
      <HelpHint term={t('machineInstallation.help.name')} what={t('machineInstallation.help.description')}
        guide={{article:'storage-cleanup',heading}} more={{label:t('help.hint.more'),onOpen:()=>openHelp('storage-cleanup',heading)}}/></div>
    {installation===null?<><Facts rows={[{name:t('machineInstallation.version'),value:version}]}/><p className="text-sm text-text-muted">{t('machineInstallation.notReported')}</p></>:<>
      <Facts rows={[
        {name:t('machineInstallation.runsAs'),value:installation.user ?? t('machines.policy.unknownSelection')},
        {name:t('machineInstallation.service'),value:installation.service === null ? t('machineInstallation.noService') : <code>{installation.service}</code>},
        {name:t('machineInstallation.binary'),value:<PathValue path={installation.executable}/>},
        {name:t('machineInstallation.dataDirectory'),value:<PathValue path={installation.data_dir}/>},
        {name:t('machineInstallation.configFile'),value:<PathValue path={installation.config_file}/>},
        {name:t('machineInstallation.version'),value:version},
      ]}/>
      {(installation.commands.uninstall===null) && <p className="text-sm text-text-muted">{t('machineInstallation.uninstallUnknown')}</p>}
      <dl className="flex flex-col gap-2">{ACTIONS.map((action)=>{const command=installation.commands[action];return typeof command!=='string'?null:<div key={action} className="grid min-w-0 grid-cols-1 items-start gap-1 sm:grid-cols-[6rem_minmax(0,1fr)] sm:gap-3">
        <dt className="pt-2 text-sm text-text-muted">{t(`machineInstallation.${action}`)}</dt><dd className="min-w-0"><CommandBox command={command} label={t('machineInstallation.copy',{action:t(`machineInstallation.${action}`)})}/></dd>
      </div>;})}</dl>
    </>}
  </section>;
}
