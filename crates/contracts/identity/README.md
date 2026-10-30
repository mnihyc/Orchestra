# orch-identity-contract

IdentityProviderKind trait seam; OIDC is the v0.1 implementation.

Implemented: the `IdentityProviderKind` trait (start a sign-in, complete it with the code, validate the provider for
**Test**), a provider's settings with the checks made when they are saved, the stored sign-in with its ten-minute
life, the callback's query and what it ends with, and the identity a provider vouches for. The rules that every kind
shares are code with tests: which claims say who somebody is, when an address counts, the domain list, the username
made from the claims, the eight callback errors, and the decision between signing in, registering, linking and
refusing. A scripted kind walks a whole sign-in through the trait in the tests; the OpenID Connect implementation is
written after this package.

| Module | Planned responsibility |
|--------|------------------------|
| [provider](src/provider.rs) | Sign-in start, callback and provider validation boundary. |
| [claims](src/claims.rs) | Verified identity claims and linking outcomes. |
