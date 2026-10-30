/** Lint static utility candidates against the same stylesheet and Tailwind implementation as the application. */
import {readFileSync,readdirSync} from 'node:fs';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import type {Rule} from 'eslint';
import {__unstable__loadDesignSystem} from 'tailwindcss';
import {classInputs} from './class-inputs.ts';

const stylesheet=fileURLToPath(new URL('../src/styles/base.css',import.meta.url));
const design=await __unstable__loadDesignSystem(readFileSync(stylesheet,'utf8'),{
  base:dirname(stylesheet),
  loadStylesheet:(id,base) => {
    const path=id.startsWith('.') ? resolve(base,id) : fileURLToPath(import.meta.resolve(id));
    return Promise.resolve({path,base:dirname(path),content:readFileSync(path,'utf8')});
  },
});
const known=new Map<string,boolean>();
const functionalRoots=design.utilities.keys('functional');

/** Read class selectors, including CSS escapes and pseudo-class arguments, without mistaking attribute values for classes. */
export function stylesheetClasses(css:string):Set<string> {
  const names=new Set<string>();
  const clean=css.replace(/\/\*[\s\S]*?\*\//g,'');
  for(const match of clean.matchAll(/(?:^|[{}])\s*([^{}]+)(?=\{)/g)) {
    const selector=match[1]??'';
    if(selector.trimStart().startsWith('@'))continue;
    let quote='',attributes=0;
    for(let index=0;index<selector.length;index+=1) {
      const character=selector[index];
      if(character==='\\'){index+=1;continue;}
      if(quote!==''){if(character===quote)quote='';continue;}
      if(character==='"'||character==="'"){quote=character;continue;}
      if(character==='['){attributes+=1;continue;}
      if(character===']'){attributes=Math.max(0,attributes-1);continue;}
      if(character!=='.'||attributes!==0)continue;
      const identifier=/^(?:\\(?:[\da-f]{1,6}\s?|[^\r\n])|[-\w\u0080-\uffff])+/i.exec(selector.slice(index+1))?.[0];
      if(identifier===undefined)continue;
      names.add(identifier.replace(/\\([\da-f]{1,6})\s?|\\([^\r\n])/gi,(_whole:string,hex:string|undefined,literal:string|undefined)=>{
        if(hex===undefined)return literal??'';
        const value=Number.parseInt(hex,16);
        // CSS escapes outside Unicode scalar values denote the replacement character.
        return String.fromCodePoint(value===0||value>0x10ffff||value>=0xd800&&value<=0xdfff ? 0xfffd : value);
      }));
      index+=identifier.length;
    }
  }
  return names;
}
function ownClasses(directory:string):Set<string> {
  const names=new Set<string>();
  for(const entry of readdirSync(directory,{withFileTypes:true})) {
    const path=resolve(directory,entry.name);
    const found=entry.isDirectory() ? ownClasses(path) : entry.isFile()&&entry.name.endsWith('.css') ? stylesheetClasses(readFileSync(path,'utf8')) : [];
    for(const name of found)names.add(name);
  }
  return names;
}
const styled=ownClasses(dirname(stylesheet));

function baseOf(candidate:string):string {
  let start=0,depth=0;
  for(let index=0;index<candidate.length;index+=1) {
    const character=candidate[index];
    if(character==='['||character==='(')depth+=1;
    if(character===']'||character===')')depth-=1;
    if(character===':'&&depth===0)start=index+1;
  }
  return candidate.slice(start).replace(/^!|!$/g,'');
}

function utilityShape(candidate:string):boolean {
  if(design.parseCandidate(candidate).some((part)=>part.kind!=='functional'||part.value!==null||part.root==='@container'))return true;
  // A bad variant must not hide an otherwise complete known utility from this gate.
  const base=baseOf(candidate).replace(/^-/,'');
  return design.utilities.has(base,'static')||functionalRoots.some((root)=>base.startsWith(root+'-')&&base.length>root.length+1);
}

/** Literal spaces inside arbitrary values belong to the candidate, rather than separating two classes. */
export function classTokens(text:string):string[] {
  const tokens:string[]=[];
  let part='',depth=0;
  for (const character of text) {
    if (/\s/.test(character)&&depth===0) {if(part!=='')tokens.push(part);part='';continue;}
    if(character==='[')depth+=1;
    if(character===']')depth=Math.max(0,depth-1);
    part+=character;
  }
  if(part!=='')tokens.push(part);
  return tokens;
}

/** Check only complete utility shapes; stylesheet classes and named group/peer markers need no generated rule. */
export function missingUtilities(text:string,defined:ReadonlySet<string>=styled):string[] {
  return classTokens(text).filter((candidate) => {
    if(candidate.endsWith('-')||candidate.endsWith(':')||defined.has(candidate)||/^(?:group|peer)(?:\/.+)?$/.test(baseOf(candidate)))return false;
    let absent=known.get(candidate);
    if(absent===undefined) {
      absent=utilityShape(candidate)&&design.candidatesToCss([candidate])[0]===null;
      known.set(candidate,absent);
    }
    return absent;
  });
}

const rule:Rule.RuleModule={
  meta:{type:'problem',schema:[],messages:{missing:'Utility "{{candidate}}" produces no CSS in the application stylesheet. Add its design token or use a built utility.'}},
  create(context) {
    const reported=new WeakMap<object,Set<string>>();
    return classInputs(context,(node,text)=>{
      const held=reported.get(node)??new Set<string>();
      for(const candidate of missingUtilities(text))if(!held.has(candidate)) {
        context.report({loc:node.loc,messageId:'missing',data:{candidate}});held.add(candidate);
      }
      reported.set(node,held);
    });
  },
};
export const utilityPlugin={rules:{'built-utilities':rule}};
