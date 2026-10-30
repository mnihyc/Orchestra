/** Each addressed list resumes its own device state; links and explicit resets replace it. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { client } from '@/api/client';
import { useListChoice, usePageListSearch } from '@/lib/listState';

const original=client.session.getState();
const validate=(state:Record<string,unknown>)=>Object.fromEntries(Object.entries(state).filter(([key])=>['view','sort','status','q','list','tab','actor','version','block','cursor'].includes(key)));
beforeEach(()=>{localStorage.clear();client.session.setState({userId:'list-reader'});});
afterEach(()=>{cleanup();client.session.setState(original);localStorage.clear();});

it('restores view, sort and filters independently for each page without restoring search words',()=>{
  const first=renderHook(()=>usePageListSearch('/project/$projectId/','/project/first',{view:'board',sort:'name',status:'done',q:'private search words'},validate));
  first.unmount();
  const second=renderHook(()=>usePageListSearch('/project/$projectId/','/project/second',{},validate));
  expect(second.result.current).not.toHaveProperty('view');second.unmount();
  const resumed=renderHook(()=>usePageListSearch('/project/$projectId/','/project/first',{},validate));
  expect(resumed.result.current).toMatchObject({view:'board',sort:'name',status:'done'});
  expect(resumed.result.current).not.toHaveProperty('q');
});

it('takes a link and Back state as a whole and remembers an explicitly cleared filter set',()=>{
  const initial=renderHook(()=>usePageListSearch('/resources/providers/','/resources/providers',{status:'disabled'},validate));initial.unmount();
  const linked=renderHook(()=>usePageListSearch('/resources/providers/','/resources/providers',{view:'mine',sort:'use',_list:true},validate));
  expect(linked.result.current).toMatchObject({view:'mine',sort:'use'});expect(linked.result.current).not.toHaveProperty('status');linked.unmount();
  const cleared=renderHook(()=>usePageListSearch('/resources/providers/','/resources/providers',{_list:true},validate));cleared.unmount();
  const resumed=renderHook(()=>usePageListSearch('/resources/providers/','/resources/providers',{},validate));
  expect(resumed.result.current).toEqual({_list:true});
});

it('clears local list controls with the existing device-data keys and keeps other lists independent',()=>{
  const one=renderHook(()=>useListChoice('first-list','sort','name'));act(()=>one.result.current[1]('use'));one.unmount();
  const other=renderHook(()=>useListChoice('second-list','sort','name'));expect(other.result.current[0]).toBe('name');other.unmount();
  const reopened=renderHook(()=>useListChoice('first-list','sort','name'));expect(reopened.result.current[0]).toBe('use');reopened.unmount();
  localStorage.clear();
  const reset=renderHook(()=>useListChoice('first-list','sort','name'));expect(reset.result.current[0]).toBe('name');
});

it('keeps content locators out of previous state and keeps a topic’s Settings filters apart from its content view',()=>{
  const document=renderHook(()=>usePageListSearch('/doc/$documentId/','/doc/one',{block:'old-block',version:4,cursor:'old-page',list:{'comments.show':'resolved'}},validate));document.unmount();
  const resumed=renderHook(()=>usePageListSearch('/doc/$documentId/','/doc/one',{},validate));
  expect(resumed.result.current).toMatchObject({list:{'comments.show':'resolved'}});expect(resumed.result.current).not.toHaveProperty('block');expect(resumed.result.current).not.toHaveProperty('version');expect(resumed.result.current).not.toHaveProperty('cursor');resumed.unmount();
  const content=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{tab:'threads'},validate));content.unmount();
  const settings=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{tab:'settings',actor:'person'},validate));settings.unmount();
  const topic=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{},validate));expect(topic.result.current).toMatchObject({tab:'threads'});expect(topic.result.current).not.toHaveProperty('actor');topic.unmount();
  const settingsAgain=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{tab:'settings'},validate));expect(settingsAgain.result.current).toMatchObject({tab:'settings',actor:'person'});
});

it('keeps unsigned list controls live without persisting another reader’s device choices',()=>{
  client.session.setState({userId:null});
  const choice=renderHook(()=>useListChoice('unsigned-list','kind','all'));
  act(()=>choice.result.current[1]('thread'));expect(choice.result.current[0]).toBe('thread');
  act(()=>choice.result.current[1]('all'));expect(choice.result.current[0]).toBe('all');
  expect(localStorage.length).toBe(0);
});
it('restores and clears nullable selected identities instead of comparing their type to null',()=>{
  const choice=renderHook(()=>useListChoice<string|null>('actor-list','actor',null));
  act(()=>choice.result.current[1]('selected-actor'));expect(choice.result.current[0]).toBe('selected-actor');choice.unmount();
  const restored=renderHook(()=>useListChoice<string|null>('actor-list','actor',null));expect(restored.result.current[0]).toBe('selected-actor');
  act(()=>restored.result.current[1](null));restored.unmount();
  expect(renderHook(()=>useListChoice<string|null>('actor-list','actor',null)).result.current[0]).toBeNull();
});
it('keeps nullable object selections live and clears anonymous state when the reader changes',()=>{
  client.session.setState({userId:null,readerVersion:1});
  const choice=renderHook(()=>useListChoice<{id:string}|null>('actor-list','person',null));
  act(()=>choice.result.current[1]({id:'selected-person'}));expect(choice.result.current[0]).toEqual({id:'selected-person'});
  act(()=>client.session.setState({userId:'another-reader',readerVersion:2}));expect(choice.result.current[0]).toBeNull();
  act(()=>client.session.setState({userId:null,readerVersion:3}));expect(choice.result.current[0]).toBeNull();
});
