//! IdentityProviderKind trait seam; OIDC is the v0.1 implementation.
//!
//! A provider lets people sign in with an account they have somewhere else. The kind of provider
//! ([`provider::IdentityProviderKind`]) is the seam: it starts a sign-in, completes it with the code the provider
//! sends back and answers with the identity the provider vouches for, and looks at the provider for **Test**.
//! Everything else is the same for every kind and is here as rules with tests: who the identity is
//! ([`claims::VerifiedIdentity`]), whether it is let in, made a user, linked, or refused with one of eight codes
//! ([`claims::decide`]), and the checks of a provider's settings and of the address to return to.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`provider`] | The trait, a provider's settings, the stored sign-in, the callback's query, and what a kind is given and answers with |
//! | [`claims`] | The identity, the names of its claims, the username made from it, the eight sign-in errors, and the decision |

#![forbid(unsafe_code)]

pub mod claims;
pub mod provider;
