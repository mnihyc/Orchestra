import { expect, it } from 'vitest';
import type { ModelSuggestion, ProviderModel } from '@/generated/api-types';
import { modelObservations, proposedModelGroup } from '@/features/resources/providers/modelOrigins';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const current: ProviderModel = { ...MODEL_ESTIMATE, id: 'source-model', model_name: 'source-model', display_name: 'My name',
  enabled: true, source: 'manual', groups: [], parameters: { input_images: true, efforts: ['low', 'high'], rate: {
    input_per_mtok: '1.00', output_per_mtok: '2', cached_input_per_mtok: '0.1',
  } } };
const observed: ModelSuggestion = { model_name: current.model_name, display_name: 'Vendor name', listed: true,
  parameters: { input_images: false, efforts: ['high', 'low'], rate: { input_per_mtok: '1', output_per_mtok: '3', cached_input_per_mtok: '0.1' } },
  origins: { '/display_name': 'vendor', '/parameters/input_images': 'vendor', '/parameters/efforts': 'vendor',
    '/parameters/rate/input_per_mtok': 'vendor', '/parameters/rate/output_per_mtok': 'catalogue',
    '/parameters/rate/cached_input_per_mtok': 'future-source' },
};

it('compares actual reported values without overwriting choices, inventing missing values or rounding prices', () => {
  const before = JSON.stringify([current, observed]);
  const fields = modelObservations(observed, current);
  expect(fields.filter((field) => field.differs).map((field) => field.field)).toEqual(['name', 'images', 'outputPrice']);
  expect(fields.find((field) => field.field === 'images')).toMatchObject({ value: false, origin: 'vendor', differs: true });
  expect(fields.find((field) => field.field === 'inputPrice')?.differs).toBe(false);
  expect(fields.find((field) => field.field === 'efforts')?.differs).toBe(false);
  expect(fields.some((field) => field.field === 'cachedPrice')).toBe(false);
  expect(JSON.stringify([current, observed])).toBe(before);
});

it('leaves legacy, future, null and missing annotations unconfirmed', () => {
  expect(modelObservations({ ...observed, origins: undefined }, current)).toEqual([]);
  expect(modelObservations({ ...observed, parameters: {}, origins: { '/parameters/input_images': 'vendor' } }, current)).toEqual([]);
  expect(modelObservations({ ...observed, parameters: { input_images: null }, origins: { '/parameters/input_images': 'vendor' } }, current)).toEqual([]);
});

it('proposes only a real vendor family accepted by the same group grammar', () => {
  const family = (line: string, origin = 'vendor'): ModelSuggestion => ({ ...observed,
    parameters: { offered: { report: { line } } }, origins: { '/parameters/offered/report/line': origin } });
  expect(proposedModelGroup(family('fixture-line'))).toBe('fixture-line');
  expect(proposedModelGroup(family('  fixture-line  '))).toBe('fixture-line');
  expect(proposedModelGroup(family('fixture-line', 'catalogue'))).toBeNull();
  expect(proposedModelGroup(family('Unconfirmed family'))).toBeNull();
  expect(proposedModelGroup(family('a'.repeat(41)))).toBeNull();
  expect(proposedModelGroup(undefined)).toBeNull();
});
