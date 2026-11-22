# Human check

A human check asks a person who has no session to prove they are not a program, before they sign in, make an
account, or have mail sent to an address. It stops a program that tries passwords without end, makes accounts
without end, or fills a mailbox with sign-in codes. Orch supports two services:

- **hCaptcha**, as a box that people tick, or unseen, asking only where it doubts;
- **Google Cloud Fraud Defense (reCAPTCHA)**, which Google's pages call reCAPTCHA and which scores people unseen.

It is **off as shipped**. While it is off, no page loads anything from outside, nothing is sent to any service, and
a system with no outside network works as it always did. The rate limits and the lock on wrong tries stay as they
are in either case; the check sits in front of them, and a person who fails it costs the name they typed no wrong try.

## What it is for, and what it is not for

It guards these acts, each of which can be left out in Management → Sign-in:

| Act | Where | Why |
|-----|-------|-----|
| Signing in | `/login`, every try, the second step included | The second step carries the password again, so a try that was not asked would be a free guess at it |
| A sign-in code by mail | "Send me a code" | It sends mail to any name |
| Registering | `/register`, and an invitation's page | A shared invitation link is as public as open registration |
| A password reset link | `/recover` | It sends mail to any address |
| The confirmation link again | "Send again" | It sends mail to any name |

It is not asked of: the first administrator at `/setup` (nothing can be switched on before there is one), signing in
through an identity provider (the provider's own sign-in is the proof), the links that carry their own secret (a
reset link, a confirmation link), API keys and tokens, and the enrolment of machines. People who are already signed
in are never asked.

Programs that sign in with a password, such as a script or a scenario run, cannot pass a check and are refused
while the act is guarded. They use an API key, which never goes through the sign-in. `orchctl admin` works on the
database and is not affected.

## Switching it on, in five steps

1. **Make a key at the service.** For hCaptcha, a site key and a secret for the address people open (its
   `[system] public_url`). For reCAPTCHA, a **score-based** key for the same address, in Google's console; the console
   shows a site key and a secret key. Checkbox keys of reCAPTCHA are not supported, and Google does not recommend them.
2. **Let the pages reach the service.** The web edge's content policy allows only its own address, and Management
   does not switch the check on while the policy the server reads at your address has no room for the service (it
   says which hosts are missing). What to do depends on where your edge's files come from:
   - **A new installation, or one whose `nginx.conf`, `frontend.sh` and `compose.yml` are those of this release's
     `orch-deploy` bundle.** Set `ORCH_FRONTEND_HUMAN_CHECK` in `.env` to `hcaptcha`, `recaptcha`, or `recaptcha-net`
     (reCAPTCHA through `www.recaptcha.net`, for people who cannot reach `www.google.com`), and run
     `docker compose up -d`. With the variable empty the policy is exactly the one the edge always sent.
   - **An installation updated with `./update.sh` from an earlier release.** An update replaces only the server, the
     application and `orchctl`; it keeps `nginx.conf`, `frontend.sh` and `compose.yml` as they were, so the variable
     does nothing there. Change the one line of `nginx.conf` that begins `add_header Content-Security-Policy` into the
     line below for your service, and run `docker compose restart frontend` (no other file changes):

     hCaptcha:

     ```
     add_header Content-Security-Policy "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-$request_id' 'wasm-unsafe-eval' https://hcaptcha.com https://*.hcaptcha.com; style-src 'self' 'unsafe-inline' https://hcaptcha.com https://*.hcaptcha.com; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self' https://hcaptcha.com https://*.hcaptcha.com; worker-src 'self'; manifest-src 'self'; frame-src 'self' https://hcaptcha.com https://*.hcaptcha.com" always;
     ```

     Google Cloud Fraud Defense (reCAPTCHA):

     ```
     add_header Content-Security-Policy "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-$request_id' 'wasm-unsafe-eval' https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self' https://www.google.com/recaptcha/; worker-src 'self'; manifest-src 'self'; frame-src 'self' https://www.google.com/recaptcha/ https://recaptcha.google.com/recaptcha/" always;
     ```

     reCAPTCHA through `www.recaptcha.net` (the setting *Use recaptcha.net*):

     ```
     add_header Content-Security-Policy "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-$request_id' 'wasm-unsafe-eval' https://www.recaptcha.net/recaptcha/ https://www.gstatic.com/recaptcha/; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self' https://www.recaptcha.net/recaptcha/; worker-src 'self'; manifest-src 'self'; frame-src 'self' https://www.recaptcha.net/recaptcha/ https://recaptcha.google.com/recaptcha/" always;
     ```

     The next release's `nginx.conf` can be taken whole as well: it starts with or without the file that
     `frontend.sh` writes, so nothing stops if `frontend.sh` stays the old one.
   - **A front of your own** (apache2, a CDN) that sets its own content policy: add the hosts in the table below to it.
     A browser enforces every policy it is given, the edge's and the front's, so each must have room.
3. **Choose the service in Management → Sign-in → Human check**, and enter its site key. For reCAPTCHA choose how it
   is asked: *SiteVerify* takes the site key and secret key the console shows and works for any key, and is the
   choice for a key made in that console; *CreateAssessment* takes a Google Cloud project and an API key restricted to
   the reCAPTCHA API, and is the way Google recommends where you have a project.
4. **Enter the secret** (hCaptcha's secret, reCAPTCHA's secret key, or the Google API key) and press **Test**. It is
   kept sealed and never shown again; **Test** asks the service with a token that is not one, and says whether the
   service was reached and accepted the keys.
5. **Look at the doctor.** `orchctl doctor --url https://your.address` has a line for the content policy
   (`edge-human-check`), and `orchctl doctor` one for the check itself (`human-check`). The doctor reads the policy
   the address gives, so a front of your own that adds or replaces a policy is seen when it answers there; it does
   not see what only some browsers are given (a policy that depends on the visitor's country, device or cookies, one
   set in the page's own markup, or a policy only for report). A `Content-Security-Policy-Report-Only` header is not
   enforced and is not read.

## What a person sees

In a box, hCaptcha's own widget stands above the button of the form, and the button is off until it is ticked. A
token serves one try: after any try, whatever came of it, the box is made fresh again and one short line says why.
Unseen, nothing is drawn; pressing the button runs the check, and under it stands the sentence the service asks of
its pages ("This site is protected by reCAPTCHA." or hCaptcha's sentence with its two links). The reCAPTCHA badge is
hidden, as Google allows when its sentence is shown.

Where the widget cannot be loaded in a person's browser (a blocker, a network that does not reach the service, a policy
with no room for it) the page says *The check could not be loaded.*, does not hold the button off, and makes the try with
`unloaded` in the place of a token. Nobody can pass a check that cannot be loaded, so the server looks into it for
itself and takes no one's word for it: where it reads the edge's policy and finds no room for the service, or cannot
ask the service either, people are let in or turned away as the setting says for a service that cannot be reached
(the audit says `edge` or the word for the vendor's failure). Where the edge has room and the service answers, a
person whose own browser cannot load the widget is asked for a check as before, since a page's word alone would let a
program say it too.

When a check is refused the page says so: *The check was not passed*, *ran out of time*, or *could not be reached*.
Where the service cannot be reached, a person is **let in and the event recorded** (the default), or turned away with
*The human check cannot be reached. Try again shortly, or ask your administrator.* when you chose
**Refuse until it can be reached**. Letting people in is the default because the form is the only way in, so an outage
or a firewall change must not shut everyone out; the limits on tries still hold.

## What is sent to the service

When a page that asks the check opens, the person's browser loads the service's script and talks to it, as the
service's own documentation describes; the service sets its own cookie (Google's is `_GRECAPTCHA`). When a person
presses a button, Orch sends the service:

- the check's token, and the secret or key, to verify it;
- **the person's network address**, as Orch resolves it from the connection and the proxies you named, so that the
  service can tell when one token is used from two places;
- for Google's `CreateAssessment`, the person's browser description and the act (`orch_sign_in`, `orch_register`, ...);
- for hCaptcha, the site key, to stop a token made for another key being used here.

Nothing else about the person is sent: no name, address, password, invitation or session. Read the terms of the service
you choose and your own privacy duties before you switch it on, and say so in your own privacy notice; hCaptcha says it
acts as a processor for you, and Google's data terms are linked from its Fraud Defense pages.

## What the administrator sees

- Management → Sign-in → Human check holds the choices. The numbers it uses (how long to wait for the service, how
  many checks one address may ask this server to make, and the window of that count) are settings with a default and
  the reason for it in their description.
- The audit trail holds, at most once an hour for each act, a row `auth.human_check_admitted` or
  `auth.human_check_refused` saying how many times the service could not be asked since the row before, and the word
  for why: `timeout`, `connect`, `status`, `unreadable`, `secret` (the service does not accept the secret), or `setup`
  (it does not know the key or the project). It holds `auth.human_check_secret_changed` when the secret is set or
  removed. No row holds a token or a secret.
- The server reads the web edge's policy at your address (and, where that cannot be reached, at the stack's own edge)
  and remembers what it found for *Time between looks*. Management → Sign-in → Human check says in one line which
  hosts the policy leaves out. Choosing the service, giving its site key, or changing the address it is loaded from is
  refused while the policy has no room for it, with *The web edge's content policy does not let pages reach this
  service yet*; once the edge is set, the same choice is let through. A choice that is already on stays as it is; if the
  edge is changed under it so that the widget cannot load, people are let in or turned away as above, so a policy
  that was wrong never shuts the forms.
- `orchctl doctor` reports the last day's events and whether the secret is kept.

## Hosts the content policy must allow

`ORCH_FRONTEND_HUMAN_CHECK` writes these, which come from each service's own documentation:

| Service | `script-src` | `frame-src` (with `'self'`) | `connect-src` | `style-src` |
|---------|--------------|-----------------------------|---------------|-------------|
| `hcaptcha` | `https://hcaptcha.com https://*.hcaptcha.com` | the same | the same | the same |
| `recaptcha` | `https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/` | `https://www.google.com/recaptcha/ https://recaptcha.google.com/recaptcha/` | `https://www.google.com/recaptcha/` | nothing |
| `recaptcha-net` | `https://www.recaptcha.net/recaptcha/ https://www.gstatic.com/recaptcha/` | `https://www.recaptcha.net/recaptcha/ https://recaptcha.google.com/recaptcha/` | `https://www.recaptcha.net/recaptcha/` | nothing |

hCaptcha asks that no subdomain be written out. The policy is the edge's for every page, not only the sign-in
pages, since a page of a single-page application keeps the policy of the first page it was loaded from; the script
itself is added to a page only where a check is asked.

## Switching it off

Choose **Off** in Management → Sign-in → Human check. Its keys and choices stay as they were. If nobody can get past
it, because the service cannot be reached and you chose to refuse, run on the host:

```
docker compose exec api orchctl admin human-check off
```

It switches the check off as the system, with an audit row. Empty `ORCH_FRONTEND_HUMAN_CHECK` and run
`docker compose up -d` to take the hosts out of the content policy again.
