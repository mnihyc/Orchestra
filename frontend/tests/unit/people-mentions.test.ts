/** Visible typed mentions retain their place and identity through drafts and edits without duplicating text or dropping files. */
import { expect, it } from 'vitest';

import { partsOfDraft } from '@/features/chat/draft';
import { draftMentionText, editedMentionNames, insertedMentionName, labelPlaceMention, typedTextParts } from '@/features/people/mentionParts';
import { candidates } from '@/features/people/MentionPicker';
import { knownPlaces } from '@/lib/placeLabel';
import type { ChatMentionName, ChatPart } from '@/generated/api-types';

const selected: ChatMentionName = { name:'Crew of Acme', target:{kind:'group',id:'crew'} };

it('same-name places in different orgs get the shared label without changing the selected role identity or count', () => {
  const known = knownPlaces([{id:'login-acme',name:'Login',above:[{id:'acme',name:'Acme'}]},{id:'login-beta',name:'Login',above:[{id:'beta',name:'Beta'}]}]);
  const target = {kind:'role' as const,node_id:'login-acme',role:'developer' as const};
  const choices = candidates([{target,name:'Developers of Login',count:4,available:true,reason:null,user:null,thread:null,agent:null,state:null,started_at:null}],0,'',(choice) => labelPlaceMention(choice.target,choice.name,known,(_role,place) => `Developers of ${place}`));
  expect(choices[1]).toMatchObject({name:'Developers of Login · Acme',display:'Developers of Login · Acme',count:4,choice:{target}});
});

it('a selected audience appears once in the middle of what was typed and returns with its current label', () => {
  const parts = typedTextParts('Please @Crew of Acme check this.', [selected]);
  expect(parts).toEqual([{type:'text',text:'Please '},{type:'mention',target:selected.target},{type:'text',text:' check this.'}]);
  expect(draftMentionText(parts, [selected])).toBe('Please @Crew of Acme check this.');
  expect(draftMentionText(parts, [{...selected,name:'Renamed crew'}])).toBe('Please @Renamed crew check this.');
  expect(typedTextParts('The audience was removed.',[selected]).some((part) => part.type === 'mention')).toBe(false);
});

it('two equal-name threads keep separate identities after reopening, inserting before, and removing the first token', () => {
  const one: ChatMentionName = {name:'Review',target:{kind:'agent',thread_id:'one'}};
  const two: ChatMentionName = {name:'Review',target:{kind:'agent',thread_id:'two'}};
  const source = '@Review and @Review please.';
  expect(typedTextParts(source,[one,two]).filter((part) => part.type === 'mention').map((part) => part.target)).toEqual([one.target,two.target]);
  const kept = editedMentionNames(source,'and @Review please.',[one,two]);
  expect(kept).toEqual([two]);
  expect(typedTextParts('and @Review please.',kept)).toContainEqual({type:'mention',target:two.target});
  const names = insertedMentionName(source,0,one,[two]);
  expect(typedTextParts(source,names).filter((part) => part.type === 'mention').map((part) => part.target)).toEqual([one.target,two.target]);
  expect(editedMentionNames('@Review please.','@Reviewer please.',[one])).toEqual([]);
});

it('a typed target round trip keeps the original quote and attachment and never treats an email as a selection', () => {
  const file: ChatPart = {type:'attachment',blob:'hash',name:'notes.txt',size:4,media_type:'text/plain'};
  const parts = partsOfDraft({text:'@Crew of Acme please read.',mentions:[selected],quotes:[{threadId:'thread',itemId:'item'}],attachments:[file]});
  expect(parts.at(-1)).toEqual(file);
  expect(parts).toContainEqual({type:'item_ref',thread_id:'thread',item_id:'item'});
  expect(typedTextParts('mail@Crew of Acme is literal.',[selected])).toEqual([{type:'text',text:'mail@Crew of Acme is literal.'}]);
});
