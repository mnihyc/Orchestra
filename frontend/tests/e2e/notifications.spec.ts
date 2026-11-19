/** How a person is told: the levels explained, what they chose about approvals, chats and quiet hours kept over a reload, every way out beginning off and a route switched on with what it carries, a way that cannot reach them yet saying what is missing and offering it, a bot connected through its link and a QR code, a webhook of their own added, changed, tested and removed, and no card where there is no way out. */
import { expect, type Page, test } from '@playwright/test';

import { botStarted, emailConfirmed, onlyChannels } from './inbox-support.ts';
import { ACME, bodySent, mailed, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const card = (page: Page, heading: string) => page.getByRole('region', { name: heading });
const mailCard = (page: Page) => card(page, 'Mail · this system');
const botCard = (page: Page) => card(page, "Telegram · Acme's bot");
const pushCard = (page: Page) => card(page, 'Push · this system');
const roomCard = (page: Page) => card(page, 'Ops room · a webhook of Acme');

async function open(page: Page): Promise<void> {
  await signIn(page);
  await page.goto('/settings/notifications');
  await expect(page.getByRole('heading', { name: 'Notifications', level: 1 })).toBeVisible();
}

test('the three levels are explained, and every way out begins off', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.getByRole('button',{name:'Notifications for this thread',exact:true}).click();
  await page.getByRole('menuitemradio',{name:'All',exact:true}).click();
  await expect(page.getByRole('button',{name:'Notifications for this thread',exact:true})).toHaveAccessibleDescription('All');
  await page.goto('/settings/notifications');
  const exceptions=page.getByRole('list',{name:'Exceptions',exact:true});
  await exceptions.getByRole('combobox').click();
  await expect(page.getByRole('option',{name:'All',exact:true})).toHaveAccessibleDescription('A turn that ends or fails, a thread that is blocked, milestones, announcements and approvals.');
  await expect(page.getByRole('option',{name:'Important',exact:true})).toHaveAccessibleDescription('A turn that fails, a thread that is blocked, approvals, and announcements marked important.');
  await page.keyboard.press('Escape');
  await expect(page.getByText('Nothing is sent to you outside Orch until you switch a way on.')).toBeVisible();
  for (const [heading, switchName] of [
    ['Mail · this system', 'Mail · this system'],
    ["Telegram · Acme's bot", "Telegram · Acme's bot"],
    ['Push · this system', 'Push · this system'],
    ['Ops room · a webhook of Acme', 'Ops room · a webhook of Acme'],
  ] as const) {
    await expect(card(page, heading).getByRole('switch', { name: switchName })).not.toBeChecked();
  }
});

test('what the person chose about approvals, chats and quiet hours is kept', async ({ page }) => {
  await open(page);
  const always = page.getByRole('switch', { name: /^Answerable approvals:/ });
  await expect(always).toBeChecked();
  await always.click();
  await expect(always).not.toBeChecked();

  await page.getByRole('combobox', { name: 'Direct and group chats' }).click();
  await page.getByRole('option', { name: 'Mentions' }).click();
  await expect(page.getByRole('combobox', { name: 'Direct and group chats' })).toContainText('Mentions');

  const quiet = page.getByRole('switch', { name: /^Quiet schedule:/ });
  await expect(quiet).not.toBeChecked();
  await quiet.click();
  // Switched on, it begins with a night: from ten to seven.
  await expect(page.getByLabel('From', { exact: true })).toHaveValue('22:00');
  await expect(page.getByLabel('Until', { exact: true })).toHaveValue('07:00');
  const saved = page.waitForResponse(async (response) => {
    if (response.request().method() !== 'PUT' || !response.url().endsWith('/me/notification-prefs') || !response.ok()) return false;
    const body = await bodySent(response.request());
    return typeof body === 'object' && body !== null && 'quiet_hours' in body && typeof body.quiet_hours === 'object' && body.quiet_hours !== null
      && 'start_minute' in body.quiet_hours && body.quiet_hours.start_minute === 23 * 60 + 30;
  });
  await page.getByLabel('From', { exact: true }).fill('23:30');
  await saved;

  await page.reload();
  await expect(page.getByRole('switch', { name: 'Answerable approvals: Followed places', exact: true })).not.toBeChecked();
  await expect(page.getByRole('combobox', { name: 'Direct and group chats' })).toContainText('Mentions');
  await expect(page.getByRole('switch', { name: 'Quiet schedule: On', exact: true })).toBeChecked();
  await expect(page.getByLabel('From', { exact: true })).toHaveValue('23:30');
  await expect(page.getByLabel('Until', { exact: true })).toHaveValue('07:00');
});

test('a route is switched on and keeps what it carries, how long a notification waits and how often a message goes', async ({ page }) => {
  await open(page);
  const mail = mailCard(page);
  await mail.getByRole('switch', { name: 'Mail · this system' }).click();
  await expect(mail.getByRole('switch', { name: 'Mail · this system' })).toBeChecked();

  // Only some kinds: the choice stands once at least one is made.
  await mail.getByRole('radio', { name: 'Only these kinds' }).click();
  await expect(mail.getByText('Choose at least one kind.')).toBeVisible();
  await mail.getByRole('button', { name: 'Choose kinds' }).click();
  await page.getByRole('checkbox', { name: 'Approval needed' }).click();
  await page.getByRole('checkbox', { name: 'Mention', exact: true }).click();
  await page.keyboard.press('Escape');
  await expect(mail.getByText('Approval needed')).toBeVisible();
  await expect(mail.getByText('Choose at least one kind.')).toHaveCount(0);

  // Both limits are unticked as a card is first switched on; ticked, they begin with the examples of the page.
  const delay = mail.getByRole('spinbutton', { name: 'Only what I have not read after' });
  await expect(delay).toBeDisabled();
  await mail.getByRole('checkbox', { name: 'Only what I have not read after' }).click();
  await expect(delay).toHaveValue('15');
  await delay.fill('20');
  await delay.blur();
  const interval = mail.getByRole('spinbutton', { name: 'Not more often than once every' });
  await mail.getByRole('checkbox', { name: 'Not more often than once every' }).click();
  await expect(interval).toHaveValue('30');
  await interval.fill('45');
  await interval.blur();

  // A number the server would refuse is said at once and sent nowhere.
  await interval.fill('2000');
  await interval.blur();
  await expect(mail.getByRole('alert')).toContainText('Enter whole minutes from 1 to 1440.');

  await page.reload();
  const again = mailCard(page);
  await expect(again.getByRole('switch', { name: 'Mail · this system' })).toBeChecked();
  await expect(again.getByText('Approval needed')).toBeVisible();
  await expect(again.getByText('Mention', { exact: true })).toBeVisible();
  await expect(again.getByRole('spinbutton', { name: 'Only what I have not read after' })).toHaveValue('20');
  await expect(again.getByRole('spinbutton', { name: 'Not more often than once every' })).toHaveValue('45');

  // Untick: at once again; and everything again.
  await again.getByRole('checkbox', { name: 'Only what I have not read after' }).click();
  await expect(again.getByRole('spinbutton', { name: 'Only what I have not read after' })).toBeDisabled();
  await again.getByRole('radio', { name: 'Everything I am notified of' }).click();
  await expect(again.getByText('Approval needed')).toHaveCount(0);
});

test('a way that cannot reach the person yet says what is missing and offers it, and is not switched on in silence', async ({ page }) => {
  await emailConfirmed(false);
  await open(page);
  const mail = mailCard(page);
  await expect(mail.getByText('Confirm ada@example.org first')).toBeVisible();
  await expect(mail.getByText('not confirmed')).toBeVisible();
  // Pressing the switch changes nothing: what is missing is already said, just below.
  await mail.getByRole('switch', { name: 'Mail · this system' }).click();
  await expect(mail.getByRole('switch', { name: 'Mail · this system' })).not.toBeChecked();
  await expect(mail.getByText('Confirm ada@example.org first')).toBeVisible();

  await mail.getByRole('button', { name: 'Send the confirmation again' }).click();
  await expect(mail.getByText('We sent the link to ada@example.org.')).toBeVisible();
  expect(await mailed('ada', 'confirm')).toContain('confirm-');

  // Confirmed meanwhile, the card is read again and offers its choices.
  await emailConfirmed(true);
  await page.reload();
  await expect(mailCard(page).getByRole('button', { name: 'Send a test' })).toBeVisible();
  await expect(mailCard(page).getByText('Confirm ada@example.org first')).toHaveCount(0);
});

test('push says that the system has no key for browsers, and offers nothing that cannot work', async ({ page }) => {
  // A browser that has not been asked.
  await page.addInitScript(() => Object.defineProperty(Notification, 'permission', { get: () => 'default', configurable: true }));
  await open(page);
  await expect(pushCard(page).getByText('Push is not set up on this system yet: it has no key for browsers.')).toBeVisible();
  await expect(pushCard(page).getByRole('button', { name: 'Allow notifications in this browser' })).toHaveCount(0);
});

test('push offers the browser’s own question where the system has a key and the browser has not been asked', async ({ page }) => {
  await scenario({ meta: { push_public_key: 'BEl62iUYgUivxIkv69yViEuiBIa-Ib9-SkvMeAtA3LFgDzkrxZJjSgSnfckjBJuBkr3qBUYIHBQFLXYp5Nksh8U' } });
  await page.addInitScript(() => Object.defineProperty(Notification, 'permission', { get: () => 'default', configurable: true }));
  await open(page);
  await expect(pushCard(page).getByText('The browser will ask you.', { exact: false })).toBeVisible();
  await expect(pushCard(page).getByRole('button', { name: 'Allow notifications in this browser' })).toBeVisible();
});

test('push in a browser that refused is told so, with where to change it', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(Notification, 'permission', { get: () => 'denied', configurable: true }));
  await open(page);
  await expect(pushCard(page).getByText("Notifications are blocked for this site. Allow them in the browser's settings, then come back.")).toBeVisible();
});

test('a bot is connected through its link: a QR code, the link, the wait for Start, and the card that reads who connected', async ({ page }) => {
  await open(page);
  const bot = botCard(page);
  await expect(bot.getByText('Not connected')).toBeVisible();
  await bot.getByRole('button', { name: 'Connect Telegram…' }).click();

  const dialog = page.getByRole('dialog', { name: 'Connect Telegram' });
  await expect(dialog.getByText('Open @acme_orch_bot and press Start. Nothing is typed on either side.')).toBeVisible();
  await expect(dialog.getByRole('img', { name: 'QR code of the link' })).toBeVisible();
  await expect(dialog.getByLabel('Link', { exact: true })).toHaveValue(/^https:\/\/t\.me\/acme_orch_bot\?start=/);
  await expect(dialog.getByRole('link', { name: 'Open the link' })).toHaveAttribute('href', /t\.me\/acme_orch_bot/);
  await expect(dialog.getByText('Waiting for you to press Start.')).toBeVisible();
  await expect(dialog.getByText(/Good until/)).toBeVisible();

  // Start is pressed in the bot: the dialog goes by itself, and the card says who connected.
  await botStarted('@alice_c');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(botCard(page).getByText('Connected as @alice_c')).toBeVisible();
  await expect(botCard(page).getByRole('button', { name: 'Send a test' })).toBeVisible();

  // Disconnected, it is as it was.
  await botCard(page).getByRole('button', { name: 'Disconnect' }).click();
  await expect(botCard(page).getByText('Not connected')).toBeVisible();
});

test('a webhook of one’s own is added, said wrong at its fields, changed, tested and removed', async ({ page }) => {
  await open(page);
  await page.getByRole('button', { name: 'Add a webhook…' }).click();
  const dialog = page.getByRole('dialog', { name: 'A webhook of my own' });
  await dialog.getByRole('button', { name: 'Add the webhook' }).click();
  await expect(dialog.getByText('Give it a name.')).toBeVisible();
  await expect(dialog.getByText('This is not a web address.')).toBeVisible();

  await dialog.getByLabel('Name').fill('My room');
  await dialog.getByLabel('Address').fill('https://hooks.example.org/mine');
  await dialog.getByLabel('Secret').fill('s3cret');
  await dialog.getByRole('button', { name: 'Add the webhook' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  const mine = card(page, 'My room · a webhook of yours');
  await expect(mine.getByRole('switch', { name: 'My room · a webhook of yours' })).not.toBeChecked();

  // The server's own refusal is shown at the field it names.
  await page.getByRole('button', { name: 'Add a webhook…' }).click();
  const second = page.getByRole('dialog', { name: 'A webhook of my own' });
  await second.getByLabel('Name').fill('My room');
  await second.getByLabel('Address').fill('https://hooks.example.org/other');
  await second.getByRole('button', { name: 'Add the webhook' }).click();
  await expect(second.getByText('is used already')).toBeVisible();
  await second.getByRole('button', { name: 'Cancel' }).click();

  // Changed: the secret that is set is said, and left empty it stays.
  await mine.getByRole('button', { name: 'Edit' }).click();
  const edit = page.getByRole('dialog', { name: 'Change My room' });
  await expect(edit.getByText('A secret is set. Leave this empty to keep it, or type a new one.')).toBeVisible();
  await edit.getByLabel('Name').fill('Our room');
  await edit.getByLabel('Address').fill('https://hooks.example.org/fail');
  await edit.getByRole('button', { name: 'Save' }).click();
  const ours = card(page, 'Our room · a webhook of yours');
  await expect(ours).toBeVisible();

  // A test says what the far side answered.
  await ours.getByRole('button', { name: 'Send a test' }).click();
  await expect(ours.getByText('Not sent: the far side answered 502')).toBeVisible();

  await ours.getByRole('button', { name: 'Remove' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Remove Our room?' });
  await expect(confirm.getByText('Nothing is sent through it any more, and every route through it ends.')).toBeVisible();
  await confirm.getByRole('button', { name: 'Remove Our room' }).click();
  await expect(card(page, 'Our room · a webhook of yours')).toHaveCount(0);
});

test('a test through a way of the org says that the far side took it, and a route of one’s own through it is switched on', async ({ page }) => {
  await open(page);
  const room = roomCard(page);
  await room.getByRole('button', { name: 'Send a test' }).click();
  await expect(room.getByText('The far side took it.')).toBeVisible();
  await room.getByRole('switch', { name: 'Ops room · a webhook of Acme' }).click();
  await expect(room.getByRole('switch', { name: 'Ops room · a webhook of Acme' })).toBeChecked();
});

test('where there is no way out there is no card, and a webhook of one’s own can still be added', async ({ page }) => {
  await onlyChannels([]);
  await open(page);
  await expect(page.getByText('No way out is set up for you here', { exact: false })).toBeVisible();
  await expect(page.getByRole('region', { name: /Mail|Telegram|Push/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Add a webhook…' })).toBeVisible();
});

test('a system with only mail has only a mail card', async ({ page }) => {
  await onlyChannels(['email']);
  await open(page);
  await expect(mailCard(page)).toBeVisible();
  await expect(botCard(page)).toHaveCount(0);
  await expect(pushCard(page)).toHaveCount(0);
});
