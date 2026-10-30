/** Chosen-org projection checks against the same fake rows that ordinary screens read. */
import { expect, test } from '@playwright/test';
import { ACME } from '../doubles/fixtures.ts';
import { Refusal, World } from '../doubles/world.ts';
const world = () => {
  const held = new World();
  const person = held.people.get('ada');
  if (person === undefined) throw new Error('the fixture needs Ada');
  return { held, person };
};


test.describe('the first-steps double reads its ordinary world', () => {
  test('requires org View even when a caller manages its members', () => {
    const { held, person } = world();
    held.places.denied.set('ada', new Set([`org.view@${ACME.org}`]));
    expect(() => held.firstSteps.projection(person, ACME.org)).toThrow(Refusal);
  });

  test('counts configuration but no final answer from models or created work', () => {
    const { held, person } = world();
    const data = held.firstSteps.projection(person, ACME.org);
    expect(data.steps.find((step) => step.id === 'provider')?.fact).toBe('key_saved');
    expect(data.steps.find((step) => step.id === 'models')?.fact).toBe('models_enabled');
    expect(data.steps.find((step) => step.id === 'work')?.fact).toBe('not_done');
    const solo = held.firstSteps.projection(person, person.me.personal_org_id);
    expect(solo.steps.find((step) => step.id === 'provider')?.fact).toBe('not_done');
    expect(solo.steps.find((step) => step.id === 'models')?.fact).toBe('not_done');
  });

  test('waits for completed nonempty final work and labels the scripted answer', () => {
    const { held, person } = world();
    for (const body of [{ commentary: true }, { incomplete: true }, { failed: true }, { text: '  ' }]) {
      held.firstSteps.control('/answer', { thread: ACME.mainThread, ...body });
      expect(held.firstSteps.projection(person, ACME.org).steps.find((step) => step.id === 'work')?.fact).toBe('not_done');
    }
    held.firstSteps.control('/answer', { thread: ACME.mainThread, scripted: true });
    const data = held.firstSteps.projection(person, ACME.org);
    expect(data.steps.find((step) => step.id === 'work')).toMatchObject({ fact: 'answered_once', state: 'done', scripted: true });
    expect(data.steps.find((step) => step.id === 'away')).toMatchObject({ state: 'unknown', fact: 'unknown', action: { url: '/settings/notifications', allowed: true } });
  });

  test('omits invitation for a saved solo choice', () => {
    const { held, person } = world();
    held.firstSteps.control('/choice', { org: person.me.personal_org_id, mode: 'solo' });
    const data = held.firstSteps.projection(person, person.me.personal_org_id);
    expect(data.work_mode).toBe('solo');
    expect(data.steps.some((step) => step.id === 'teammate')).toBe(false);
  });

  test('updates role summaries from changed defaults while keeping model access separate', () => {
    const { held, person } = world();
    const before = held.firstSteps.projection(person, ACME.org).invitation_roles;
    expect(before.find((entry) => entry.role === 'developer')).toMatchObject({ start_topics: true, send_inputs: true, model_access_required: true });
    const row = held.access.pageFor(ACME.org, true).rows.get('project.interact|any');
    if (row === undefined) throw new Error('the fixture needs project defaults');
    row.inherit = false;
    row.entries = [];
    expect(held.firstSteps.projection(person, ACME.org).invitation_roles.find((entry) => entry.role === 'developer')).toMatchObject({ start_topics: false, send_inputs: true, model_access_required: true });
  });

  test('returns only a current readable document reference and rejects a guide from another root', async () => {
    const { held, person } = world();
    const created = held.firstSteps.control('/document', { org: ACME.org, title: 'Welcome' })?.body as { id: string };
    const chosen = held.firstSteps.projection(person, ACME.org).start_here;
    expect(chosen).toMatchObject({ id: created.id, title: 'Welcome' });
    expect(chosen).not.toHaveProperty('blocks');
    const other = held.firstSteps.control('/document', { org: person.me.personal_org_id, title: 'Private elsewhere' })?.body as { id: string };
    await expect(held.firstSteps.handle(person, 'PUT', `/nodes/${ACME.org}/settings/org.start_here_document_id`, new URLSearchParams(), { value: other.id })).rejects.toThrow(Refusal);
    held.firstSteps.control('/document-change', { id: created.id, title: 'Renamed welcome', hidden: true });
    const viewer = held.people.get('vera');
    if (viewer === undefined) throw new Error('the fixture needs Vera');
    const hidden = held.firstSteps.projection(viewer, ACME.org);
    expect(hidden.start_here).toBeNull();
    expect(JSON.stringify(hidden)).not.toContain(created.id);
    expect(JSON.stringify(hidden)).not.toContain('Renamed welcome');
    held.firstSteps.control('/document-change', { id: created.id, deleted: true });
    expect(held.firstSteps.projection(person, ACME.org).start_here).toBeNull();
  });
});
