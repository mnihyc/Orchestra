//! The suite every identity provider kind passes: start, callback, claims and refusal codes.
//!
//! The provider fixture must be a disposable issuer. Successful completion and nonce refusal use
//! independently issued authorization codes so a one-use code cannot mask a broken nonce check.

use std::time::Duration;

use crate::report::{Report, skip};
use orch_identity_contract::claims::VerifiedIdentity;
use orch_identity_contract::provider::{CompleteRequest, IdentityProviderKind, StartRequest};

/// Discovery and one token exchange are each bounded by one minute.
const WITHIN: Duration = Duration::from_secs(60);

/// Requests and independently minted codes from a controlled issuer.
pub struct Fixture {
    /// The settings and callback for starting a sign-in.
    pub start: StartRequest,
    /// A fresh code and its genuine pending state.
    pub complete: CompleteRequest,
    /// The identity that the controlled issuer signed.
    pub identity: VerifiedIdentity,
    /// Another fresh code paired with an intentionally wrong nonce.
    pub wrong_nonce: CompleteRequest,
}

/// Checks discovery, fresh sign-in correlation, verified claims and a false nonce's refusal.
pub async fn run(kind: &dyn IdentityProviderKind, fixture: Option<&Fixture>, report: &mut Report) {
    let Some(fixture) = fixture else {
        report.checked(
            "identity.fixture",
            "A disposable identity provider is exercised",
            Err(skip(
                "no controlled issuer and authorization codes supplied",
            )),
        );
        return;
    };
    report.checked(
        "identity.settings",
        "The valid issuer settings pass their field checks",
        kind.check(&fixture.start.settings)
            .map_err(|faults| format!("{} invalid provider fields", faults.len()).into()),
    );
    report
        .run(
            "identity.discovery",
            "Discovery confirms the configured issuer and its signing keys",
            WITHIN,
            async {
                let discovered = kind
                    .validate(&fixture.start.settings)
                    .await
                    .map_err(|error| error.to_string())?;
                if discovered.issuer.trim_end_matches('/')
                    != fixture.start.settings.issuer.trim_end_matches('/')
                    || discovered.signing_keys <= 0
                {
                    return Err("discovery named another issuer or no usable signing key".into());
                }
                Ok(())
            },
        )
        .await;
    report
        .run(
            "identity.start",
            "Two sign-ins have separate nonempty state, nonce and verifier",
            WITHIN,
            async {
                let first = kind
                    .start(&fixture.start)
                    .await
                    .map_err(|error| error.to_string())?;
                let second = kind
                    .start(&fixture.start)
                    .await
                    .map_err(|error| error.to_string())?;
                if first.pending.state.is_empty()
                    || first.pending.nonce.is_empty()
                    || first.pending.pkce_verifier.is_empty()
                    || first.pending.state == second.pending.state
                    || first.pending.nonce == second.pending.nonce
                    || first.pending.pkce_verifier == second.pending.pkce_verifier
                    || !first.redirect_url.contains(&first.pending.state)
                {
                    return Err(
                        "sign-in requests share correlation values or omit them from the redirect"
                            .into(),
                    );
                }
                Ok(())
            },
        )
        .await;
    report
        .run(
            "identity.claims",
            "The signed identity is returned without changing its claims",
            WITHIN,
            async {
                let identity = kind
                    .complete(&fixture.complete)
                    .await
                    .map_err(|error| error.to_string())?;
                if identity == fixture.identity {
                    Ok(())
                } else {
                    Err("the verified identity differs from the issuer's claims".into())
                }
            },
        )
        .await;
    report
        .run(
            "identity.nonce",
            "A valid signed identity with the wrong nonce is refused",
            WITHIN,
            async {
                if fixture.complete.code == fixture.wrong_nonce.code {
                    return Err(
                        "use independent one-use codes for the positive and false-nonce cases"
                            .into(),
                    );
                }
                if kind.complete(&fixture.wrong_nonce).await.is_err() {
                    Ok(())
                } else {
                    Err("the provider accepted a token belonging to another sign-in".into())
                }
            },
        )
        .await;
}
