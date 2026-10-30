/** Stored storage filters are categorical, validated and separate from pagination and locator text. */
import { expect,it } from 'vitest';
import { patchStorageSearch,readStorageSearch,storageRequest } from '@/features/storage/search';

it('keeps server and unknown breakdown buckets without storing typed locator text',()=>{
  expect(readStorageSearch({host:'server',owner:'unknown',kind:'image',q:'private typed words',by:'owner'})).toEqual({host:'server',owner:'unknown',kind:'image',by:'owner'});
  expect(storageRequest({by:'owner',host:'server',kind:'image'})).toEqual({host:'server',kind:'image'});
});
it('changing a filter clears the old cursor and invalid saved choices fall back',()=>{
  expect(patchStorageSearch({cursor:'old',kind:'image',by:'host'},{why:'cache'})).toEqual({kind:'image',by:'host',why:'cache'});
  expect(readStorageSearch({scope:'someone-else',kind:'missing-kind',by:'missing',owner:'bad-id',age:'missing'})).toEqual({});
});
