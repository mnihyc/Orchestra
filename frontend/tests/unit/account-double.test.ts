// @vitest-environment node
/** The account double preserves omitted fields, requires address proof, registers as the real API does and keeps the last allowed sign-in path. */
import { describe, expect, it } from 'vitest';
import { Refusal, World } from '../doubles/world.ts';
import { ACME } from '../doubles/fixtures.ts';

describe('account request outcomes',() => {
  it('accepts a chat invitation as a distinct outsider without opening the surrounding org', () => {
    const world = new World();
    const existing = new Set([...world.people.values()].map((person) => person.me.id));
    expect(world.chats.control('/guest', {}).status).toBe(200);
    const guest = world.people.get('gus')!;
    expect(existing.has(guest.me.id)).toBe(false);
    const made = world.chats.control('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Release guests' });
    const chat = (made.body as { chat: string }).chat;
    world.inviteToChat('guest-invitation', chat, 'ada');
    const accepted = world.accept(guest, { token: 'guest-invitation' });
    expect(accepted.status).toBe(200);
    expect(accepted.body).toMatchObject({ org: null, node: null, chat: { id: chat, kind: 'group', org_id: ACME.org } });
    expect(world.places.isMember(guest, ACME.org)).toBe(false);
    expect(() => world.tree(guest, ACME.org, null)).toThrow(Refusal);
    expect(world.chats.handle(guest, 'GET', `/chats/${chat}`, new URLSearchParams(), {})?.body).toMatchObject({ id: chat, guests: [guest.me.id] });
    const displayed = world.peopleWork.handle(guest, 'GET', `/chats/${chat}`, new URLSearchParams(), {});
    expect(displayed?.status).toBe(200);
    expect((displayed?.body as { inspection_holders: unknown[] }).inspection_holders.length).toBeGreaterThan(0);
    expect(() => world.access.handle(guest, 'GET', `/nodes/${ACME.org}/permissions`, new URLSearchParams(), {})).toThrow(Refusal);
    world.chats.handle(world.people.get('ada')!, 'DELETE', `/chats/${chat}/members/${guest.me.id}`, new URLSearchParams(), {});
    expect(() => world.peopleWork.handle(guest, 'GET', `/chats/${chat}`, new URLSearchParams(), {})).toThrow(Refusal);
  });
  it('keeps a deliberate fixed UTC choice when a delayed device report arrives, then lets the person follow this device again', () => {
    const world = new World();
    const ada = world.people.get('ada')!;
    ada.me = { ...ada.me, timezone: 'UTC', timezone_mode: 'device' };
    world.updateMe(ada, { timezone: 'UTC' });
    expect(ada.me).toMatchObject({ timezone: 'UTC', timezone_mode: 'fixed' });
    world.updateMe(ada, { timezone: 'Asia/Singapore', timezone_mode: 'device', timezone_report: true });
    expect(ada.me).toMatchObject({ timezone: 'UTC', timezone_mode: 'fixed' });
    world.updateMe(ada, { timezone: 'Asia/Singapore', timezone_mode: 'device' });
    expect(ada.me).toMatchObject({ timezone: 'Asia/Singapore', timezone_mode: 'device' });
    world.updateMe(ada, { timezone: 'Europe/London', timezone_mode: 'device', timezone_report: true });
    expect(ada.me).toMatchObject({ timezone: 'Europe/London', timezone_mode: 'device' });
  });
  it('keeps an omitted address, resets colour on null, and clears only with the right password',() => {
    const world=new World();
    const ada=world.people.get('ada')!;
    const original=ada.me.email;
    world.updateMe(ada,{avatar_color:4});
    expect(ada.me.avatar.color).toBe(4);
    expect(ada.me.email).toBe(original);
    world.updateMe(ada,{avatar_color:null});
    const reset=ada.me.avatar.color;
    expect(reset).toBeGreaterThanOrEqual(0);
    expect(reset).toBeLessThan(12);
    expect(() => world.updateMe(ada,{email:null,current_password:'wrong'})).toThrow(Refusal);
    expect(ada.me.email).toBe(original);
    world.updateMe(ada,{email:null,current_password:ada.password});
    expect(ada.me.email).toBeNull();
    expect(ada.me.email_pending).toBeNull();
    expect(ada.me.email_verified).toBe(false);
  });
  it('refuses clearing when mail is the last way in or the mailed second step is still enabled',() => {
    const world=new World();
    const dora=world.people.get('dora')!;
    dora.me={...dora.me,has_password:false,identities:[]};
    expect(() => world.updateMe(dora,{email:null})).toThrow(Refusal);
    expect(dora.me.email).toBe('dora@example.org');
    dora.me={...dora.me,has_password:true,email_code_second_step:true};
    expect(() => world.updateMe(dora,{email:null,current_password:dora.password})).toThrow(Refusal);
    expect(dora.me.email).toBe('dora@example.org');
  });
  it('checks only public names and gives confirmation registrations no session, even for an existing address',() => {
    const world=new World({meta:{registration_mode:'open'},confirmAddresses:true});
    expect(world.nameAvailability('ada').body).toEqual({available:false});
    expect(world.nameAvailability('acme').body).toEqual({available:false});
    expect(() => world.nameAvailability('ada@example.org')).toThrow(Refusal);
    const before=world.sessions.size;
    const outcome=world.register({username:'new-person',display_name:'New Person',password:'a sufficiently long secret',email:'ada@example.org'});
    expect(outcome.status).toBe(202);
    expect(outcome.body).toEqual({verification_required:true});
    expect(outcome.cookies).toBeUndefined();
    expect(world.sessions.size).toBe(before);
  });
  it('registers as the real API does: closed takes nobody, a team invitation admits only when accepted, and the answer says where it leads',() => {
    const identity={display_name:'Dana Scully',password:'a sufficiently long secret'};
    // The double refuses as the server's handlers do: by throwing the answer.
    const refused=(act: () => unknown): string => {
      try { act(); } catch (error) { if (error instanceof Refusal) return `${error.answer.status} ${(error.answer.body as {code:string}).code}`; throw error; }
      return 'answered';
    };
    const closed=new World({meta:{registration_mode:'closed'}});
    expect(refused(() => closed.register({...identity,username:'dana',invitation:'good-invitation',accept_invitation:true}))).toBe('403 registration.closed');
    // The system takes people by invitation, as it ships.
    const world=new World();
    expect(refused(() => world.register({...identity,username:'dana',invitation:'good-invitation'}))).toBe('403 registration.invite_required');
    expect(refused(() => world.register({...identity,username:'dana',accept_invitation:true}))).toBe('400 request.invalid');
    expect(refused(() => world.register({...identity,username:'dana',invitation:'spent-invitation',accept_invitation:true}))).toBe('403 registration.invite_required');
    expect(world.people.has('dana')).toBe(false);
    const joined=world.register({...identity,username:'dana',invitation:'good-invitation',accept_invitation:true});
    expect(joined.status).toBe(201);
    expect((joined.body as {accepted_invitation:{org:{slug:string}|null}|null}).accepted_invitation?.org?.slug).toBe('acme');
    expect(world.people.get('dana')?.me.personal_org_id).not.toBeNull();
  });
  it('makes no personal org where the system makes none',() => {
    const world=new World({meta:{registration_mode:'open'}});
    world.admin.own.set('orgs.personal_enabled',{value:false,locked:false,revision:1});
    const made=world.register({username:'erin',display_name:'Erin',password:'a sufficiently long secret'});
    expect(made.status).toBe(201);
    expect((made.body as {accepted_invitation:unknown}).accepted_invitation).toBeNull();
    expect(world.people.get('erin')?.me.personal_org_id).toBeNull();
  });
});
