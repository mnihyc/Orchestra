// @vitest-environment node
/** Class inputs produce CSS or use own styles, while metadata stays outside the utility gate. */
import {Linter} from 'eslint';
import tseslint from 'typescript-eslint';
import {expect,it} from 'vitest';
import {classTokens,missingUtilities,stylesheetClasses,utilityPlugin} from '../../build/utility-classes';

it('builds the spacing, list heights and tutorial widths that the screens use',() => {
  expect(missingUtilities('gap-1.5 px-1.5 size-3.5 size-7 min-h-9 w-14 h-20 min-w-32 max-h-48 max-h-56 max-h-60 max-h-64 max-h-72 max-h-96 w-64 w-72 w-96')).toEqual([]);
  expect(missingUtilities('gap-99 max-h-99')).toEqual(['gap-99','max-h-99']);
  expect(missingUtilities('unshipped:gap-2')).toEqual(['unshipped:gap-2']);
});
it('keeps variants and arbitrary values intact, and accepts semantic marker classes',() => {
  expect(classTokens("hover:gap-1.5 after:content-['two words'] w-[min(22rem,calc(100vw-2rem))]")).toEqual(['hover:gap-1.5',"after:content-['two words']",'w-[min(22rem,calc(100vw-2rem))]']);
  expect(missingUtilities('group group/card peer focus-ring @container gap-2')).toEqual([]);
});

it('builds the actual missing warning/link, tracking, query, panel and motion utilities',()=>{
  expect(missingUtilities('text-warning-text text-link tracking-wide [&_[cmdk-group-heading]]:tracking-wide @3xl:grid-rows-1 @3xl:grid-cols-2 @3xl:flex-row @sm:inline @md:inline max-w-sm max-w-md w-xl! rounded-xs animate-pulse bg-white bg-yellow-200/50 hover:bg-black/20')).toEqual([]);
});

it('accepts nested stylesheet selectors and escaped names, without reading attribute values as class selectors',()=>{
  const own=stylesheetClasses(String.raw`@layer components { .text-owned, :is(.bg-owned\:a) {color:red} @media (width > 20rem) { .gap-99 {gap:1px} } [data-value=".text-not-a-class"] {color:blue} }`);
  expect([...own].sort()).toEqual(['bg-owned:a','gap-99','text-owned']);
  expect(missingUtilities('text-owned gap-99',own)).toEqual([]);
  expect(missingUtilities('text-not-a-class',own)).toEqual(['text-not-a-class']);
});

function complaints(source:string):string[] {
  const messages=new Linter().verify(source,[{
    files:['**/*.tsx'],languageOptions:{parser:tseslint.parser,parserOptions:{ecmaVersion:'latest',sourceType:'module',ecmaFeatures:{jsx:true}}},
    plugins:{orch:utilityPlugin},rules:{'orch/built-utilities':'error'},
  }],{filename:'classes.tsx'});
  expect(messages.filter((message)=>message.ruleId!=='orch/built-utilities')).toEqual([]);
  return messages.map((message)=>/Utility "([^"]+)"/.exec(message.message)?.[1]??message.message);
}

it('ignores ordinary values, MIME types, labels, identifiers, style values and class-like data attributes',()=>{
  expect(complaints(`
    const words=['text','object','end','right','list','row','left','accent','start','text/csv','text/plain','start-options','right-bar','me-state','bg-not-a-utility-here'];
    const classes=['start-options','right-bar','bg-not-a-class'];
    const schema={class:'bg-not-a-class',type:'object',classes:['start-options']};
    const {classes:entries=['start-options']}=schema;
    const component=<Pane label="bg-not-a-class" data-testid="outline-panel" side="right" style={{gridRow:'row-before'}} className="gap-2 group/entry peer/control"/>;
  `)).toEqual([]);
  expect(missingUtilities('text object end right list row left accent start text/csv text/plain')).toEqual([]);
});

it('reads class attributes, shared className props, conditional outputs and helper object keys, but not their conditions',()=>{
  expect(complaints(`
    const side='start-options';
    const component=<><div class="gap-99"/><Pane fieldClassName={side==='start-options'?'h-99':'h-4'} rowClassName={(row)=>row.id==='text/plain'?'w-99':'w-4'}/></>;
    const classes=clsx({'max-h-99':side==='right-bar'},side==='row-before'&&'min-w-99');
  `).sort()).toEqual(['gap-99','h-99','max-h-99','min-w-99','w-99']);
});

it('follows local constants, indexed class maps and class-producing functions with their lexical bindings',()=>{
  expect(complaints(`
    const defaults={label:'bg-not-class-text',control:'w-99'};
    const options={...defaults,control:'gap-99'};
    const TONES={quiet:'h-99',strong:'h-4'};
    function box(active){if(active)return 'max-h-99';return 'max-h-4';}
    const component=<div className={cx(options.control,TONES[tone],box(active))}/>;
  `).sort()).toEqual(['gap-99','h-99','max-h-99']);
});

it('also validates the shared exported class helpers where their class strings are defined',()=>{
  expect(complaints(`export const fieldBoxClasses='gap-99';export function buttonClasses(){return 'h-99';}`).sort()).toEqual(['gap-99','h-99']);
});

it('reads selected tuple entries and mapping outputs, rather than unused labels or mapping inputs',()=>{
  expect(complaints(`
    const tuple=['bg-metadata-label','gap-99'];
    const states=['row-before','row-after'];
    const component=<div className={cx(tuple[1],states.map((state)=>state==='row-before'?'h-99':'h-4').join(' '))}/>;
  `).sort()).toEqual(['gap-99','h-99']);
});

it('recognizes renamed class helpers and imperative DOM class writes, without treating toggle conditions as classes',()=>{
  expect(complaints(`
    import {cx as join} from '@/ui/cx';
    const component=<div className={join('gap-99')}/>;
    element.classList.toggle('h-99',state==='bg-not-a-class');
    element.classList.add('w-99');element.setAttribute('class','min-h-99');
    element.className='max-h-99';
  `).sort()).toEqual(['gap-99','h-99','max-h-99','min-h-99','w-99']);
});
