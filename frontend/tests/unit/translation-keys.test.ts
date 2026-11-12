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
import { translationKeys, textAt, present, type UsedKey } from '../../build/translation-keys.ts';

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
  it('rejects an unknown authored key through aliases, finite choices and Trans without a type checker', () => {
    const source = [
      "const fixed = 'common.misspelled'; t(fixed);",
      "const prefix = 'common'; t(`${prefix}.alsoMissing`);",
      "function row(kind: 'next' | 'notAKnownKey') { return t(`common.${kind}`); }",
      "(['back', 'anotherMissing'] as const).map((kind) => t(`common.${kind}`));",
      "for (const kind of ['add', 'oneMoreMissing'] as const) t(`common.${kind}`);",
      "const words = { ready: 'common.next', broken: 'common.missingInMap' }; t(words[state]);",
      "const english = i18n.getFixedT('en'); english('common.fixedMissing');",
      "const { t: say } = useTranslation(); say('common.aliasMissing');",
      '<Trans i18nKey={ok ? "common.next" : "common.transMissing"} />',
      "t2('common.secondTranslatorMissing');",
      "t('common.missingEvenWithFallback', { defaultValue: 'Readable fallback' });",
    ].join('\n');
    const requested = translationKeys(source);
    const missing = requested.filter((key) => !presentForSource('screen.tsx', key, 'en')).map((key) => key.key);
    expect(missing).toEqual(['common.misspelled', 'common.alsoMissing', 'common.notAKnownKey', 'common.anotherMissing',
      'common.oneMoreMissing', 'common.missingInMap', 'common.fixedMissing', 'common.aliasMissing', 'common.transMissing',
      'common.secondTranslatorMissing', 'common.missingEvenWithFallback']);
    expect(requested.filter((key) => presentForSource('screen.tsx', key, 'en')).map((key) => key.key))
      .toEqual(['common.next', 'common.back', 'common.add', 'common.next', 'common.next']);
  });

  it('checks every member of finite indexed and destructured local sets', () => {
    const source = [
      "const names=['common.next','common.indexMissing']; t(names[index]);",
      "const rows=[{key:'common.back'},{key:'common.objectMissing'}]; rows.map((entry)=>t(entry.key));",
      "rows.map(({key})=>t(key));",
      "for (const {key} of rows) t(key);",
      "const {key: selected}={key:'common.bindingMissing'}; t(selected);",
      "const nested={first:{title:'common.next'},last:{title:'common.nestedMissing'}}; t(nested[index].title);",
    ].join('\n');
    expect(translationKeys(source).filter((key) => !presentForSource('screen.tsx', key, 'en')).map((key) => key.key))
      .toEqual(['common.indexMissing','common.objectMissing','common.objectMissing','common.objectMissing','common.bindingMissing','common.nestedMissing']);
  });

  it('rejects a misspelled dynamic family without guessing native values', () => {
    const used = translationKeys('t(`common.${native}`); t(`commmon.${native}`); t(native);', 'screen.tsx', true);
    expect(used.map((key) => key.key)).toEqual(['common.${…}', 'commmon.${…}']);
    expect(used.map((key) => presentForSource('screen.tsx', key, 'en'))).toEqual([true, false]);
    const flat = translationKeys('t(`zzzzNoSuchKeyPrefix${native}`); t(`${native}`);', 'screen.tsx', true);
    expect(flat.map((key) => key.key)).toEqual(['zzzzNoSuchKeyPrefix${…}', '${…}']);
    expect(flat.map((key) => presentForSource('screen.tsx', key, 'en'))).toEqual([false, true]);
  });

  it('keeps object and context readers strict while native templates check fixed parts only', () => {
    const rows = translationKeys("t('labels',{returnObjects:true}); t('unknown',{returnObjects:true}); t('row',{context:native,defaultValue:'fallback'}); t('missing',{context:native,defaultValue:'fallback'});", 'screen.tsx', true);
    expect(rows.map((key) => present({ labels: { name: 'Name' }, row_ready: 'Ready' }, key, 'en'))).toEqual([true,false,true,false]);
    expect(rows.map((key) => present({ labels: { name: 'Name' }, rowboat_ready: 'Wrong family' }, key, 'en'))).toEqual([true,false,false,false]);
    const native = translationKeys("const kind = typeof value.kind==='string'?value.kind:''; t(`event.${kind}`);", 'screen.tsx', true);
    expect(native.map((key) => key.key)).toEqual(['event.${…}']);
    expect(present({event:{ready:'Ready'}}, native[0]!, 'en')).toBe(true);
    const mixed = translationKeys("function field(name:'title'|'description'|'name') { t(`driver.${native}.${name}`); }", 'screen.tsx', true);
    expect(mixed.map((key) => key.key)).toEqual(['driver.${…}.${…}']);
    expect(present({driver:{sample:{title:'Name'}}}, mixed[0]!, 'en')).toBe(true);
  });

  it('does not read a shadowed binding, a cycle, comments or an unrelated literal as a translation', () => {
    const source = "const key='common.outerMissing'; function row(key:string) { t(key); } "
      + "{ const key='common.next'; t(key); } const first=second; const second=first; t(first); "
      + "const cache='common.cacheMissing'; // t('common.commentMissing')";
    expect(translationKeys(source).map((key) => key.key)).toEqual(['common.next']);
  });

  it('has every literal t key in English and Chinese, including a call with its own defaultValue',()=>{
    const failures:string[]=[];
    for(const file of sourceFiles(root))for(const used of translationKeys(readFileSync(file,'utf8'),file,true)){
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
