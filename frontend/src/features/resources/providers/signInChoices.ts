/** A process being installed never implies that its profile has reported subscription support. */
import type { TFunction } from 'i18next';
import type { ProviderSetup } from '@/generated/api-types';
import type { ChoiceDetails } from '@/ui/Choice';
import { resourcePreview } from '@/features/resource-facts/ResourceFacts';
import { programName } from '@/features/resources/programNames';

export interface SignInChoice extends ChoiceDetails {
  value:string;
  label:string;
  disabled:boolean;
  description?:string;
}

export function signInChoices(setup:ProviderSetup|undefined,t:TFunction,locale:string):SignInChoice[] {
  if(setup===undefined)return [];
  const machines=setup.machines ?? [];
  const choices:SignInChoice[]=setup.subscriptions.map((row)=>{
    const name=programName(row.harness,locale) ?? t('resources.harnessNameUnknown');
    const blocked=row.agent.machine!==null && setup.machines!==undefined && !machines.some((choice)=>
      choice.machine.id===row.agent.machine?.id && choice.roles.includes('agent') && choice.harnesses.includes(row.harness.kind));
    return {value:row.agent.id,label:row.agent.name===name?row.agent.name:`${row.agent.name} (${name})`,disabled:blocked,preview:resourcePreview({kind:'agent',agent:row.agent}),
      ...(blocked?{description:t('providerCreation.subscriptionMachineUnavailable')}:{})};
  });
  const unavailable=setup.subscriptions_unavailable ?? [];
  for(const row of unavailable){
    const name=programName(row.harness,locale) ?? t('resources.harnessNameUnknown');
    choices.push({value:`unavailable:${row.agent?.id ?? row.harness.kind}`,label:row.agent===null || row.agent.name===name
      ?name:`${row.agent.name} (${name})`,disabled:true,
      description:t(`providerCreation.subscriptionUnavailable.${row.reason}`),...(row.agent===null?{}:{preview:resourcePreview({kind:'agent',agent:row.agent})})});
  }
  // Older setup replies still name unread profiles separately. Do not show the same profile twice.
  for(const agent of setup.subscriptions_unreported ?? []){
    if(!unavailable.some((row)=>row.agent?.id===agent.id))choices.push({value:`unavailable:${agent.id}`,label:agent.name,disabled:true,
      description:t('providerCreation.subscriptionUnreported'),preview:resourcePreview({kind:'agent',agent})});
  }
  for(const row of setup.subscriptions_withheld){
    const value=`withheld:${row.harness.kind}`;
    if(!choices.some((choice)=>choice.value===value))choices.push({value,label:programName(row.harness,locale) ?? t('resources.harnessNameUnknown'),disabled:true,
      description:t('providerCreation.subscriptionWithheld')});
  }
  return choices;
}
