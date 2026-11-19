/** Account controls send explicit colour/reset/clear intent; registration and invitations open only their returned outcomes. */
import { expect, type Page, test } from '@playwright/test';
import type { Me } from '../../src/generated/api-types.ts';
import { chats } from './chat-support.ts';
import { ACME, bodySent, expectAccessibleInBothThemes, PEOPLE, replySent, scenario, signIn } from './support.ts';

const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
async function steer(path:string,body:unknown) {
  const response=await fetch(`${DOUBLE}/__double${path}`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
  expect(response.ok).toBeTruthy();
}
async function identity(page:Page,username:string) {
  await page.getByLabel('Username',{exact:true}).fill(username);
  await page.getByLabel('Name',{exact:true}).fill('New Person');
  await page.getByLabel('Password',{exact:true}).fill('a sufficiently long secret');
}
test.beforeEach(async () => { await scenario(); });

test('a palette choice changes the shared avatar, and reset sends null to use the server’s colour',async ({page}) => {
  await signIn(page,PEOPLE.ada);
  await page.goto('/settings/profile');
  const colour=page.getByRole('radio',{name:'Colour 5',exact:true});
  await colour.click();
  await expect(colour).toBeChecked();
  await expect(page.getByRole('img',{name:'How your picture looks'})).toHaveClass(/bg-\(--identity-4\)/);
  await page.goto('/u/ada');
  await expect(page.getByRole('complementary',{name:'About this person'}).locator('.avatar').first()).toHaveClass(/bg-\(--identity-4\)/);
  await page.goto('/settings/profile');
  const reset=page.waitForResponse((response) => response.url().endsWith('/api/v1/me') && response.request().method() === 'PATCH');
  await page.getByRole('button',{name:'Reset avatar colour'}).click();
  const resetReply=await reset;
  expect(await bodySent(resetReply.request())).toEqual({avatar_color:null});
  const returned=await resetReply.json() as Me;
  await expect(page.getByRole('radio',{name:`Colour ${returned.avatar.color+1}`,exact:true})).toBeChecked();
  await expectAccessibleInBothThemes(page);
});

test('removing the address proves the password and keeps the address when that proof fails',async ({page}) => {
  await signIn(page,PEOPLE.ada);
  await page.goto('/settings/profile');
  await page.getByRole('button',{name:'Remove address',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Remove your address?'});
  await dialog.getByLabel('Your password', { exact: true }).fill('not the password');
  await dialog.getByRole('button',{name:'Remove address',exact:true}).click();
  await expect(dialog.getByRole('alert')).toBeVisible();
  await expect(page.getByText('ada@example.org',{exact:true})).toBeVisible();
  await dialog.getByLabel('Your password', { exact: true }).fill(PEOPLE.ada.password);
  const clearing=page.waitForResponse((response) => response.url().endsWith('/api/v1/me') && response.request().method() === 'PATCH');
  await dialog.getByRole('button',{name:'Remove address',exact:true}).click();
  expect(await bodySent((await clearing).request())).toEqual({email:null,current_password:PEOPLE.ada.password});
  await expect(dialog).toHaveCount(0);
  await expect(page.getByText('No address yet.')).toBeVisible();
  await page.reload();
  await expect(page.getByText('No address yet.')).toBeVisible();
});

test('the last available sign-in and a mailed second step each explain why removal is unavailable',async ({page}) => {
  await signIn(page,PEOPLE.dora);
  await steer('/settings/account',{username:'dora',has_password:false});
  await page.goto('/settings/profile');
  await expect(page.getByRole('button',{name:'Remove address',exact:true})).toBeDisabled();
  await expect(page.getByText(/^Keep another allowed way to sign in before removing/)).toBeVisible();
  await steer('/settings/account',{username:'dora',has_password:true,email_code_second_step:true});
  await page.reload();
  await expect(page.getByRole('button',{name:'Remove address',exact:true})).toBeDisabled();
  await expect(page.getByText('Switch off the mailed second step in Security before removing your address.')).toBeVisible();
});

test('public-name availability finds a root slug and a registration conflict stays on its named field',async ({page}) => {
  await scenario({meta:{registration_mode:'open'}});
  const checked:string[]=[];
  page.on('request',(request) => {const url=new URL(request.url()); if (url.pathname.endsWith('/auth/name-availability')) checked.push(url.searchParams.get('name') ?? '');});
  await page.goto('/register');
  await identity(page,'acme');
  await expect(page.getByRole('status').filter({hasText:'A username or root org already uses this public name.'})).toBeVisible();
  await page.getByRole('button',{name:'Create account'}).click();
  await expect(page.getByLabel('Username',{exact:true})).toHaveAttribute('aria-invalid','true');
  await expect(page.getByLabel('Username',{exact:true})).toBeFocused();
  await page.getByLabel('Username',{exact:true}).fill('new-person');
  await expect(page.getByRole('status').filter({hasText:'This public name is available now.'})).toBeVisible();
  await page.getByLabel('Email address').fill('ada@example.org');
  await page.getByLabel('Name',{exact:true}).focus();
  expect(checked).toContain('acme');
  expect(checked).toContain('new-person');
  expect(checked.some((name) => name.includes('@'))).toBe(false);
});

test('201 registration accepts the node invitation and Continue opens that returned topic',async ({page}) => {
  await page.goto('/register?invitation=topic-invitation');
  await identity(page,'new-person');
  // Acceptance is part of the registration itself: no second call accepts the invitation afterwards.
  const separately:string[]=[];
  page.on('request',(request) => { if (new URL(request.url()).pathname.endsWith('/invitations/accept')) separately.push(request.method()); });
  const registered=page.waitForResponse((response) => response.url().endsWith('/auth/register'));
  await page.getByRole('button',{name:'Create account and accept invitation',exact:true}).click();
  const answer=await registered;
  expect(answer.status()).toBe(201);
  // The answer names what was accepted, which the server does only when the registration asked for it (the request's body
  // cannot be read here: the client sends it as a stream).
  expect(await replySent(answer)).toMatchObject({accepted_invitation:{node:{id:ACME.fixLogin}}});
  await page.getByRole('button',{name:'Continue',exact:true}).click();
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}$`));
  await expect(page.getByRole('heading',{name:'Fix login',exact:true,level:1})).toBeVisible();
  expect(separately).toEqual([]);
});

test('202 registration shows confirmation without adopting a session or accepting an invitation',async ({page}) => {
  await scenario({meta:{registration_mode:'open'},confirmAddresses:true});
  await page.goto('/register');
  await identity(page,'new-person');
  await page.getByLabel('Email address').fill('new-person@example.org');
  const registered=page.waitForResponse((response) => response.url().endsWith('/auth/register'));
  await page.getByRole('button',{name:'Create account'}).click();
  const response=await registered;
  expect(response.status()).toBe(202);
  expect(await replySent(response)).toEqual({verification_required:true});
  await expect(page).toHaveTitle(/^Confirm your address/);
  expect((await page.context().cookies()).some((cookie) => cookie.name === 'orch_session')).toBe(false);
  await expect(page.getByRole('button',{name:'Your account'})).toHaveCount(0);
});

test('an accepted group invitation opens the real guest messages route and no org work',async ({page}) => {
  const made=await chats('/chat',{org:ACME.org,members:['ada','dora'],name:'Release guests'});
  expect(typeof made.chat).toBe('string');
  await steer('/invitation/chat',{token:'group-invitation',chat:made.chat,as:'ada'});
  // The regular seeded accounts already belong to Acme; this scenario needs an actual outsider.
  const guest=await chats('/guest',{});
  expect(guest.username).toBe('gus');
  await signIn(page,{login:'gus',password:'a guest has a long password'});
  await page.goto('/invite/group-invitation');
  await page.getByRole('button',{name:'Accept invitation'}).click();
  await expect(page).toHaveURL(new RegExp(`/org/${ACME.org}/messages\\?chat=${String(made.chat)}$`));
  await expect(page.getByRole('heading',{name:'Release guests'})).toBeVisible();
  await expect(page.getByRole('button',{name:'New message',exact:true})).toHaveCount(0);
  await expect(page.getByText('Upgrade deps',{exact:true})).toHaveCount(0);
});
