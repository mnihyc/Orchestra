// @vitest-environment node
/** Every literal translation requested by shipped source exists in each catalogue, without a language fallback. */
import {readFileSync,readdirSync} from 'node:fs';
import {relative,resolve} from 'node:path';
import ts from 'typescript';
import {describe,expect,it} from 'vitest';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';
import developmentEnglish from '@/i18n/dev-en.json';
import developmentChinese from '@/i18n/dev-zh-CN.json';

const root=resolve(import.meta.dirname,'../../src');
/** Match the existing source-audit suites' 20-second ceiling; parse each authored file once for both languages. */
const SOURCE_AUDIT_MS=20_000;
function sourceFiles(directory:string):string[]{
  return readdirSync(directory,{withFileTypes:true}).flatMap((entry)=>{
    const path=resolve(directory,entry.name);
    // Generated contracts and the generated route tree contain data, not authored translation calls.
    if(entry.isDirectory())return entry.name==='generated'?[]:sourceFiles(path);
    return /\.tsx?$/.test(entry.name) && !entry.name.endsWith('.d.ts') && entry.name!=='routeTree.gen.ts'?[path]:[];
  });
}
function unwrapped(node:ts.Expression):ts.Expression{
  return ts.isAsExpression(node)||ts.isTypeAssertionExpression(node)||ts.isParenthesizedExpression(node)||ts.isSatisfiesExpression(node)?unwrapped(node.expression):node;
}
function literalKeys(given:ts.Expression):string[]{
  const node=unwrapped(given);
  if(ts.isStringLiteralLike(node))return [node.text];
  if(ts.isConditionalExpression(node))return [...literalKeys(node.whenTrue),...literalKeys(node.whenFalse)];
  if(ts.isArrayLiteralExpression(node))return node.elements.flatMap((entry)=>ts.isOmittedExpression(entry)?[]:literalKeys(entry));
  // A parameter/computed template belongs to the exhaustive registry/choice tests; never invent its values here.
  return [];
}
interface UsedKey{key:string;line:number;count:boolean|null;context:string|null;ordinal:boolean}
function translationKeys(text:string,file='screen.tsx'):UsedKey[]{
  const source=ts.createSourceFile(file,text,ts.ScriptTarget.Latest,true,file.endsWith('.tsx')?ts.ScriptKind.TSX:ts.ScriptKind.TS);
  const found:UsedKey[]=[];
  const visit=(node:ts.Node)=>{
    if(ts.isCallExpression(node)){
      const expression=node.expression;
      const translates=ts.isIdentifier(expression)?expression.text==='t':ts.isPropertyAccessExpression(expression)&&expression.name.text==='t';
      const first=node.arguments[0],options=node.arguments[1];
      if(translates && first!==undefined){
        const fields=options!==undefined && ts.isObjectLiteralExpression(options)?options.properties:[];
        const field=(name:string):ts.Expression|undefined=>{
          for(const entry of fields){
            if(!ts.isPropertyAssignment(entry)&&!ts.isShorthandPropertyAssignment(entry))continue;
            const key=ts.isIdentifier(entry.name)||ts.isStringLiteralLike(entry.name)?entry.name.text:entry.name.getText(source);
            if(key===name)return ts.isShorthandPropertyAssignment(entry)?entry.name:entry.initializer;
          }
          return undefined;
        };
        const context=field('context');
        const count=field('count')!==undefined?true:options!==undefined && (!ts.isObjectLiteralExpression(options)||options.properties.some(ts.isSpreadAssignment))?null:false;
        for(const key of literalKeys(first))found.push({key,line:source.getLineAndCharacterOfPosition(node.getStart(source)).line+1,count,ordinal:field('ordinal')?.kind===ts.SyntaxKind.TrueKeyword,context:context!==undefined && ts.isStringLiteralLike(context)?context.text:null});
      }
    }
    ts.forEachChild(node,visit);
  };
  visit(source);return found;
}
function textAt(catalogue:unknown,path:string):string|null{
  let value=catalogue;
  for(const key of path.split('.'))value=typeof value==='object'&&value!==null&&!Array.isArray(value)?(value as Record<string,unknown>)[key]:undefined;
  return typeof value==='string'&&value.trim()!==''?value:null;
}
function present(catalogue:unknown,used:UsedKey,language:'en'|'zh-CN'):boolean{
  const keys=used.context===null?[used.key]:[`${used.key}_${used.context}`,used.key];
  for(const key of keys){
    if(textAt(catalogue,key)!==null)return true;
    if(used.count===false)continue;
    const plural=used.ordinal?`${key}_ordinal`:key;
    if((language==='en'?['one','other']:['other']).every((form)=>textAt(catalogue,`${plural}_${form}`)!==null))return true;
  }
  return false;
}

/** The development route explicitly installs its own bilingual bundle; production source cannot borrow from it. */
function presentForSource(file:string,used:UsedKey,language:'en'|'zh-CN'):boolean{
  const ordinary=language==='en'?english:chinese;
  if(present(ordinary,used,language))return true;
  return file.startsWith('routes/dev/') && used.key.startsWith('dev.')
    && present(language==='en'?developmentEnglish:developmentChinese,used,language);
}

/** JSON.parse silently keeps the last duplicate, which can undo a translated action during a merge. */
function duplicateKeys(text:string):string[]{
  const source=ts.createSourceFile('catalogue.json',text,ts.ScriptTarget.Latest,true,ts.ScriptKind.JSON);
  const found:string[]=[];
  const visit=(node:ts.Node,path:string[])=>{
    if(ts.isObjectLiteralExpression(node)){
      const seen=new Set<string>();
      for(const property of node.properties){
        if(!ts.isPropertyAssignment(property)||!ts.isStringLiteralLike(property.name))continue;
        const key=property.name.text,next=[...path,key];
        if(seen.has(key))found.push(next.join('.'));
        seen.add(key);visit(property.initializer,next);
      }
    }else ts.forEachChild(node,(child)=>visit(child,path));
  };
  visit(source,[]);return found;
}

describe('translation keys used by the product',()=>{
  it('rejects duplicate catalogue keys before JSON parsing can replace the first wording',()=>{
    expect(duplicateKeys('{"isolation":{"replaced":"first","replaced":"second"}}')).toEqual(['isolation.replaced']);
    expect(duplicateKeys('{"first":{"key":"one"},"second":{"key":"two"}}')).toEqual([]);
    for(const language of ['en','zh-CN']){
      const duplicates=duplicateKeys(readFileSync(resolve(root,'i18n',`${language}.json`),'utf8'));
      expect(duplicates,`${language}: ${duplicates.join(', ')}`).toEqual([]);
    }
  });

  it('reads calls, conditionals, casts, key lists and plural options while ignoring comments, prose and computed keys',()=>{
    const text=["// t('comment.missing')",'const prose="t(\'prose.missing\')";',"t('common.next');",'env.t(`common.back`);',"t(ok ? 'common.add' : 'common.cancel');",'t(`dynamic.${value}`);',"t(['common.next','common.back']);","t('rows' as TranslationKey,{count:3});"].join('\n');
    const keys=translationKeys(text);
    expect(keys.map((entry)=>entry.key)).toEqual(['common.next','common.back','common.add','common.cancel','common.next','common.back','rows']);
    const counted=keys.find((entry)=>entry.key==='rows');
    if(counted===undefined)throw new Error('The plural call must be collected');
    expect(present({rows_one:'One row',rows_other:'Rows'},counted,'en')).toBe(true);
    expect(present({rows_one:'One row'},counted,'en')).toBe(false);
    const first=keys[0];
    if(first===undefined)throw new Error('The first literal call must be collected');
    expect(present({},first,'en')).toBe(false);
    expect(present({common:{next:'Next'}},{...first,context:'optional'},'en')).toBe(true);
    const fallback=translationKeys("t('missing.key',{defaultValue:'A readable fallback'});")[0];
    if(fallback===undefined)throw new Error('A fallback call must still be collected');
    expect(present({},fallback,'en')).toBe(false);
    expect(translationKeys("t('rows',{count});")[0]?.count).toBe(true);
  });
  it('has every literal t key in English and Chinese, including a call with its own defaultValue',()=>{
    const failures:string[]=[];
    for(const file of sourceFiles(root))for(const used of translationKeys(readFileSync(file,'utf8'),file)){
      const path=relative(root,file).replaceAll('\\','/');
      for(const language of ['en','zh-CN'] as const)if(!presentForSource(path,used,language))failures.push(`${path}:${used.line} ${language}:${used.key}`);
    }
    expect(failures, failures.join('\n')).toEqual([]);
  },SOURCE_AUDIT_MS);
  it('requires the development bundle in each language without making it a production fallback',()=>{
    const key=translationKeys("t('dev.title');")[0];
    if(key===undefined)throw new Error('The development call must be collected');
    for(const language of ['en','zh-CN'] as const){
      expect(presentForSource('routes/dev/ui/-Gallery.tsx',key,language)).toBe(true);
      expect(presentForSource('features/resources/ResourceList.tsx',key,language)).toBe(false);
      expect(presentForSource('routes/dev/ui/-Gallery.tsx',{...key,key:'dev.missing'},language)).toBe(false);
    }
  });
  it('keeps the compact Add and provider wizard actions readable in both languages',()=>{
    for(const [catalogue,expected] of [[english,['Next','Back','Add']],[chinese,['下一步','上一步','添加']]] as const){
      expect(['common.next','common.back','common.add'].map((key)=>textAt(catalogue,key))).toEqual([...expected]);
    }
  });
});
