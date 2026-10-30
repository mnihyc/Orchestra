# orch-identity-oidc

OpenID Connect identity-provider implementation.

The kind `oidc` signs people in with the authorization code flow: the issuer's discovery document
and keys are read from the issuer (and read again at once when a token names a key that is not kept,
so a rotation is followed), every sign-in carries its own state, nonce and PKCE verifier (`S256`), the
code is exchanged from the server, and the ID token is believed only after its signature (RSA, ECDSA
or Ed25519, against the published keys), issuer, audience, times and nonce passed. **Test** reads the
discovery document and the keys anew and says what it found. Unit tests cover the token and key
rules with literal documents and keys signed in the test; integration tests run whole sign-ins against
the signed local provider of the test kit, faults included.

| Module | Planned responsibility |
|--------|------------------------|
| [discovery](src/discovery.rs) | Issuer metadata and provider validation. |
| [authorization](src/authorization.rs) | State, nonce, PKCE and sign-in redirects. |
| [callback](src/callback.rs) | Verified identity claims and callback outcomes. |
| [jws](src/jws.rs) | Compact signed tokens and the published keys that verify them. |
