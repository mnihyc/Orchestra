/** Operation presets are snapshots, with independent actions and no cross-target widening. */
import { describe, expect, it } from 'vitest';
import { KEY_ACTIONS, offeredActions, presetOperations, ruleOperations, selectOperations } from '@/features/settings/keyRules';
import type { KeyOperation, Operation, OperationFacet } from '@/generated/api-types';

const facet = (action: OperationFacet['action'], usage: OperationFacet['usage'] = 'required'): OperationFacet => ({ action, usage, resources: ['document'], target: { kind: 'permission_object' } });
const operation = (operationId: string, facets: OperationFacet[], delegation: Operation['delegation'] = 'business'): Operation => ({
  operation_id: operationId, method: 'POST', path: '/documents/{id}/ops', title: operationId, delegation,
  variants: [{ operation: operationId, name: '', selector: 'all', legacy_tokens: true, facets }],
});
const catalogue = [operation('DocumentGet.read', [facet('read')]), operation('DocumentOpsCreate', [facet('write'), facet('read', 'response_content')])];

describe('explicit operation-key selections', () => {
  it('keeps Write independent of optional response-content Read, regardless of HTTP method', () => {
    expect(presetOperations(catalogue, ['document'], ['write'])).toEqual([{ operation: 'DocumentOpsCreate', actions: ['write'] }]);
    expect(presetOperations(catalogue, ['document'], ['read'])).toEqual([{ operation: 'DocumentGet.read', actions: ['read'] }, { operation: 'DocumentOpsCreate', actions: ['read'] }]);
  });

  it('never offers public, protocol or browser-elevation operations as business grants', () => {
    const all = [...catalogue, operation('Public', [facet('read')], 'public'), operation('Auth', [facet('write')], 'authentication_protocol'), operation('Elevate', [facet('manage')], 'browser_elevation')];
    expect(presetOperations(all, ['document'], KEY_ACTIONS).map((item) => item.operation)).toEqual(['DocumentGet.read', 'DocumentOpsCreate']);
    expect(ruleOperations(all, []).length).toBe(0);
  });

  it('retains every required compound facet in review while selecting only the chosen resource actions', () => {
    const compound = operation('DocumentTreeOperate.copy', [facet('read'), { action: 'write', usage: 'required', resources: ['document_container'], target: { kind: 'resolved', value: 'destination' } }]);
    expect(ruleOperations([compound], ['document'])[0]?.variants[0]?.facets).toEqual(compound.variants[0]?.facets);
    expect(offeredActions(compound.variants[0]!, ['document'])).toEqual(['read']);
    expect(presetOperations([compound], ['document_container'], ['write'])).toEqual([{ operation: 'DocumentTreeOperate.copy', actions: ['write'] }]);
  });

  it('changes one rule without mutating another rule that selected the same operation', () => {
    const one: KeyOperation[] = [{ operation: 'DocumentOpsCreate', actions: ['read'] }];
    const two: KeyOperation[] = [{ operation: 'DocumentOpsCreate', actions: ['write'] }];
    const next = selectOperations(two, catalogue[1]!.variants, ['document'], ['manage', 'read'], true);
    expect(next).toEqual([{ operation: 'DocumentOpsCreate', actions: ['read', 'write'] }]);
    expect(one).toEqual([{ operation: 'DocumentOpsCreate', actions: ['read'] }]);
    expect(two).toEqual([{ operation: 'DocumentOpsCreate', actions: ['write'] }]);
  });

  it('preserves unavailable saved IDs and does not implicitly select a new catalogue operation', () => {
    const value: KeyOperation[] = [{ operation: 'PreviouslySelected', actions: ['write'] }];
    const expanded = [...catalogue, operation('NewOperation', [facet('write')])];
    const next = selectOperations(value, expanded[0]!.variants, ['document'], ['read'], true);
    expect(next).toEqual([{ operation: 'PreviouslySelected', actions: ['write'] }, { operation: 'DocumentGet.read', actions: ['read'] }]);
    expect(next.some((item) => item.operation === 'NewOperation')).toBe(false);
  });

  it('uses the same Manage selection for provider reset and authorization operations', () => {
    const providerFacet: OperationFacet = { action: 'manage', usage: 'required', resources: ['provider'], target: { kind: 'permission_object' } };
    const rows = [operation('ProviderResetConsume', [providerFacet]), operation('ProviderAuthorizationCreate', [providerFacet])];
    expect(presetOperations(rows, ['provider'], ['manage'])).toEqual([{ operation: 'ProviderResetConsume', actions: ['manage'] }, { operation: 'ProviderAuthorizationCreate', actions: ['manage'] }]);
  });
});
