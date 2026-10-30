/** Leaving Programs retains its declared names in every profile picker, without another request. */
import { expect,it } from 'vitest';
import { readMachineFacts,registrableHarnesses } from '@/features/resources/machines/facts';
import { machineProgramName,reportedAdapterName } from '@/features/resources/machines/programNames';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

it('takes the declared adapter kind and title independently from its registration alias',()=>{
  const descriptor={kind:'custom-model-agent',harness:{name:'Research assistant'},sandbox_modes:['workspace','open']};
  const facts={programs:{entries:[{kind:'agent',name:'gpu-adapter',ready:true,descriptor}]},adapters:[{kind:'custom-model-agent',needs_binary:false}],harnesses:[]};
  expect(registrableHarnesses(readMachineFacts(facts))).toEqual([{kind:'custom-model-agent',title:'Research assistant',path:'',version:'',daemon:false}]);
  expect(machineProgramName({name:'gpu-adapter',descriptor})).toBe('Research assistant');
  expect(reportedAdapterName({...facts,programs:{entries:[{kind:'agent',name:'gpu-adapter',ready:false,descriptor}]}},'custom-model-agent')).toBeNull();
  expect(reportedAdapterName({plugins:[{kind:'adapter',name:'gpu-adapter',state:'running',descriptor}]},'custom-model-agent')).toBe('Research assistant');
  expect(reportedAdapterName({programs:{entries:{kind:'agent'}},plugins:[null,'not a declaration',{kind:'adapter',name:'gpu-adapter',state:'running',descriptor}]},'custom-model-agent')).toBe('Research assistant');
  expect(reportedAdapterName({programs:{entries:[null,42]},plugins:'not a list'},'custom-model-agent')).toBeNull();
});

it('keeps the provider form navigation words present in both shipped catalogues',()=>{
  for(const catalogue of [english,chinese])for(const key of ['next','back','add'] as const)expect(catalogue.common[key].trim()).not.toBe('');
  expect(english.common.next).toBe('Next');expect(chinese.common.next).toBe('下一步');
});
