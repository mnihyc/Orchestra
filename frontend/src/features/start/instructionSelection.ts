/** Whether the selected source brings its shared instructions to the new workspace. */
import type { FileSelection } from '@/generated/api-types';
const NAME='AGENTS.md';

/** The wire grammar gives only * and ? pattern meaning; braces and brackets are literal. */
function matches(pattern:string,path:string) {
  if(pattern==='')return true;
  let expression='';
  for(let at=0;at<pattern.length;at++) {
    const char=pattern[at];
    if(char==='*'&&pattern[at+1]==='*') {
      at++;
      if(pattern[at+1]==='/'){expression+='(?:.*/)?';at++;}else expression+='.*';
    }else if(char==='*')expression+='[^/]*';
    else if(char==='?')expression+='[^/]';
    else expression+=(char??'').replace(/[.*+?^${}()|[\]\\]/gu,'\\$&');
  }
  return new RegExp(`^${expression}${pattern.includes('*')||pattern.includes('?')?'':'(?:/.*)?'}$`,'u').test(path);
}

export function includesInstructions(selection:FileSelection|null|undefined,directory='') {
  const root=directory.replace(/\/+$/u,'');
  const path=root===''?NAME:`${root}/${NAME}`;
  if(selection===null||selection===undefined||selection.mode==='all')return true;
  if(selection.mode==='none')return false;
  return selection.include.some((pattern)=>matches(pattern,path))&&!(selection.exclude??[]).some((pattern)=>matches(pattern,path));
}
