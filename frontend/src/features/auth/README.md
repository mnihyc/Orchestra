# Setup, registration, signing in by each proof, and invitations

`recoveryCode.ts` supplies the same short next step after a saved second factor on sign-in or session verification. The requested destination remains open; a persistent Security action offers fresh replacement of a lost authenticator's backups. Setup and replacement sheets keep plaintext only while open, with metadata-only caches.

The system's product name is the page's only h1. An AuthCard keeps its purpose in the browser tab;
it does not repeat the form's action as a heading. This applies to setup, registration, recovery and
second-step forms as well as sign-in. Footer preferences show short language/theme names. A connection
failure gives its actual cause once, a concrete connection/address step and Retry; diagnostic details
remain available.

The screens a person meets before they are signed in: the first administrator, signing in by each
proof with its second step and its refusals, registration as the system allows it, invitations,
recovery, a new password and the confirmation of an address. Each works against the API's requests of
authentication and is exercised by the end-to-end tests in a real browser, with an accessibility
check of every screen in both themes. What a sign-in through an identity provider comes back with is
put into words; linking an identity from one's profile belongs to the settings and is not here.

Sign-in begins with the username and password, its visibility control and submit button. Recovery,
mailed-code sign-in and resending are offered only when the system can send mail. Expired links
without mail explain how to ask an administrator; a reset link handed out by an administrator still
works. Invitation-only registration without a valid invitation returns to sign-in, which explains
how to get an invitation. The second step names the account and retains the server's saved-recovery-code
choice; unavailable mailed proofs never appear. Provider choices follow an "or" divider. When local
password sign-in is off, providers appear without that form or divider; the explicit administrator
entry still opens its restricted password form. Invitations and registration lead into this same
sign-in flow when the person chooses to sign in.

Registration checks the public username/root-org namespace after typing rests; addresses are never
availability queries. Structured name conflicts stay on that field, and other account conflicts stay
generic. A session is adopted only for the registration reply that carries one; a confirmation reply
stays outside. Both invitation entry points open the returned readable node, permitted chat or joined org.

- `Setup.tsx`: Create the first administrator.
- `Login.tsx`: Sign-in: identity providers, a password, a code sent by mail where somebody may use one, the second step as the answer names it, and a refusal that says which proof is closed and until when.
- `EmailCode.tsx`: The code sent by mail: asking for one, the one field for it, and the same sentence whatever the name.
- `Register.tsx`: Registration as the system allows it: by an invitation, openly, or not at all; a personal link asking for the address it was sent to.
- `Recovery.tsx`: Mailed recovery where available; otherwise the administrator's next step.
- `ResetPassword.tsx`: Reset with a token delivered by mail or an administrator.
- `VerifyEmail.tsx`: Email verification.
- `Invitation.tsx`: What an invitation is for and until when, before anything is asked; one that is no longer good says so and no more.
- `accepted.ts`: An accepted invitation opens only its returned readable target, including a chat-only guest's real messages route.
- `IdentityCallback.tsx`: Sign-in and identity-link callback outcomes.
- `Frame.tsx`: The page the screens before sign-in stand on: the system's name and notice, one column for the form, and the language and theme at its foot.
- `freshProof.ts`: A fresh proof of a person without a password: their identity provider proves them again for one act, and comes back with a one-use proof in the fragment, the form's draft kept meanwhile.
- `SecondStepGate.tsx`: What a page shows in place of an org's work that waits for the person's second step: Set up a second step, or Verify your second step.
- `identityRules.ts`: What a username, a name, a password and an address must look like before they are sent, said in words beside the field.
