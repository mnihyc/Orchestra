//! Looking at a proof: a password, a code that was mailed, and a code of an authenticator app, each counted by itself; and what a set of right proofs lacks of a sign-in.
//!
//! A proof is looked at only after its try has been counted ([`throttle`]),
//! and it is looked at the same way whether the name is somebody's or nobody's:
//! a password is checked against a hash that is there or a decoy, a mailed
//! code against the code that is there or none.
//!
//! What completes a sign-in depends on the person. With no second step, one
//! proof that can be a first step does: the password always, a mailed code
//! where their orgs allow it. With a second step, two different proofs do,
//! one of which is a first step and one a second: a password with an
//! authenticator's code, a password with a mailed code that they switched on
//! as their second step, or a mailed code with an authenticator's code. One
//! mailbox is one proof however many codes it reads, so a person whose only
//! second step is the mailed code, and who began with a mailed code, is asked
//! for their password.

use super::throttle::{self, Who};
use super::{Limits, codes, passwords, people};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_api_contract::auth::SignInProof;
use orch_api_contract::users::EmailCodeUse;
use orch_db::Conn;
use orch_db::queries::identity::{self, UserRow};

/// What a person's way into the system allows, as of now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// An authenticator is their second step.
    pub totp: bool,
    /// At least one unspent backup of that enabled authenticator exists.
    pub recovery: bool,
    /// A mailed code may get them in by itself.
    pub mailed_first: bool,
    /// A mailed code is their second step: their orgs allow it, and they
    /// switched it on.
    pub mailed_second: bool,
}

impl Capabilities {
    /// Whether the person has a second step of either kind.
    #[must_use]
    pub fn has_second(&self) -> bool {
        self.totp || self.mailed_second
    }

    /// Whether `proof` can be the first step of their sign-in.
    #[must_use]
    pub fn first(&self, proof: SignInProof) -> bool {
        match proof {
            SignInProof::Password => true,
            SignInProof::EmailCode => self.mailed_first,
            SignInProof::Totp | SignInProof::RecoveryCode => false,
        }
    }

    /// Whether `proof` can be their second step.
    #[must_use]
    pub fn second(&self, proof: SignInProof) -> bool {
        match proof {
            SignInProof::Totp => self.totp,
            SignInProof::RecoveryCode => self.recovery,
            SignInProof::EmailCode => self.mailed_second,
            SignInProof::Password => false,
        }
    }

    /// Whether the proofs that were right complete the sign-in.
    #[must_use]
    pub fn complete(&self, right: &[SignInProof]) -> bool {
        if !self.has_second() {
            return right.iter().any(|proof| self.first(*proof));
        }
        right.iter().any(|first| {
            self.first(*first)
                && right
                    .iter()
                    .any(|second| second != first && self.second(*second))
        })
    }

    /// The proofs of which one more would complete a sign-in that has the
    /// proofs `right`, in the order the contract lists them. Nothing is asked
    /// for from a person who has no second step: what they have given either
    /// completes it or is no way in for them.
    #[must_use]
    pub fn missing(&self, right: &[SignInProof]) -> Vec<SignInProof> {
        if !self.has_second() {
            return Vec::new();
        }
        [
            SignInProof::Totp,
            SignInProof::RecoveryCode,
            SignInProof::EmailCode,
            SignInProof::Password,
        ]
        .into_iter()
        .filter(|proof| !right.contains(proof))
        .filter(|proof| {
            let mut with = right.to_vec();
            with.push(*proof);
            self.complete(&with)
        })
        .collect()
    }
}

/// What the person's way into the system allows now: what their orgs allow of
/// a mailed code, and what they switched on.
pub async fn capabilities(conn: &mut Conn, user: &UserRow) -> ApiResult<Capabilities> {
    let recovery = user.totp_on()
        && orch_db::queries::recovery_codes::status(conn, user.id)
            .await?
            .remaining
            > 0;
    capabilities_with_recovery(conn, user, recovery).await
}

/// The same method rules when the caller already read backup metadata on this connection.
pub async fn capabilities_with_recovery(
    conn: &mut Conn,
    user: &UserRow,
    recovery: bool,
) -> ApiResult<Capabilities> {
    let uses = people::email_code_uses(conn, Some(user)).await?;
    Ok(Capabilities {
        totp: user.totp_on(),
        recovery: user.totp_on() && recovery,
        mailed_first: uses.contains(&EmailCodeUse::SignIn),
        mailed_second: user.email_code_on() && uses.contains(&EmailCodeUse::SecondStep),
    })
}

/// Counts and looks at a password given for the name `who`: a wrong one is a
/// wrong try, and the same is true whether the name is somebody's or not.
pub async fn password_right(
    ctx: &Ctx,
    conn: &mut Conn,
    limits: &Limits,
    who: &Who,
    user: Option<&UserRow>,
    given: &str,
    by_address: bool,
) -> ApiResult<bool> {
    let tried = throttle::begin(ctx, conn, limits, who, SignInProof::Password, by_address).await?;
    let right = passwords::verify(given, user.and_then(|user| user.password_hash.as_deref())).await;
    throttle::end(ctx, conn, limits, who, tried, right).await?;
    Ok(right)
}

/// Counts and looks at a code that was mailed. The answer is the hash of the
/// code if it is the one the person has, good now; the code is spent when the
/// sign-in is complete, and not before.
pub async fn mailed_code_right(
    ctx: &Ctx,
    conn: &mut Conn,
    limits: &Limits,
    who: &Who,
    user: Option<&UserRow>,
    given: &str,
    by_address: bool,
) -> ApiResult<Option<Vec<u8>>> {
    let tried = throttle::begin(ctx, conn, limits, who, SignInProof::EmailCode, by_address).await?;
    let live = identity::live_sign_in_code(conn, user.map(|user| user.id), ctx.now).await?;
    let id = user.map_or_else(orch_types::Id::nil, |user| user.id);
    let matches = codes::matches(live.as_ref().map(|row| row.code_hash.as_slice()), id, given);
    let address_is_current = live.as_ref().zip(user).is_some_and(|(row, user)| {
        user.email_confirmed()
            && user
                .email
                .as_deref()
                .is_some_and(|email| email.trim().to_lowercase() == row.email.trim().to_lowercase())
    });
    let right = matches && address_is_current;
    throttle::end(ctx, conn, limits, who, tried, right).await?;
    Ok(live.filter(|_| right).map(|row| row.code_hash))
}

/// Counts and looks at a code of the person's authenticator. The answer is
/// the time step of the code if it is a code of theirs that was not used yet;
/// the step is accepted when the sign-in is complete, and not before.
pub async fn authenticator_right(
    ctx: &Ctx,
    conn: &mut Conn,
    limits: &Limits,
    who: &Who,
    user: &UserRow,
    given: &str,
    by_address: bool,
) -> ApiResult<Option<i64>> {
    let tried = throttle::begin(ctx, conn, limits, who, SignInProof::Totp, by_address).await?;
    let step = step_of(ctx, user, given)?;
    throttle::end(ctx, conn, limits, who, tried, step.is_some()).await?;
    Ok(step)
}

/// Checks one recovery code after a valid first factor, without spending it yet. Its attempts
/// share the authenticator's limits; only the completed act may consume the returned hash.
pub async fn recovery_right(
    ctx: &Ctx,
    conn: &mut Conn,
    limits: &Limits,
    who: &Who,
    user: &UserRow,
    given: &str,
    by_address: bool,
) -> ApiResult<Option<Vec<u8>>> {
    let tried = throttle::begin(
        ctx,
        conn,
        limits,
        who,
        SignInProof::RecoveryCode,
        by_address,
    )
    .await?;
    let normalized = super::recovery_codes::normalize(given);
    let hash = super::recovery_codes::hash(user.id, normalized.as_deref().unwrap_or_default());
    let available = orch_db::queries::recovery_codes::available(conn, user.id, &hash).await?;
    let right = user.totp_on() && normalized.is_some() && available;
    throttle::end(ctx, conn, limits, who, tried, right).await?;
    Ok(right.then_some(hash))
}

/// The time step of `given`, where it is a code of the authenticator of
/// `user` at this moment and a code of its step was not accepted before.
fn step_of(ctx: &Ctx, user: &UserRow, given: &str) -> ApiResult<Option<i64>> {
    let Some(sealed) = user.totp_secret.as_deref() else {
        return Ok(None);
    };
    let secret = codes::open_secret(user.id, sealed).map_err(ApiError::internal)?;
    let totp = codes::authenticator(secret, &user.username, &ctx.app.config.system.name)
        .map_err(ApiError::internal)?;
    Ok(codes::totp_step(&totp, given, ctx.now)
        .filter(|step| user.totp_last_step.is_none_or(|last| *step > last)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use SignInProof::{EmailCode, Password, Totp};

    fn can(totp: bool, mailed_first: bool, mailed_second: bool) -> Capabilities {
        Capabilities {
            totp,
            recovery: false,
            mailed_first,
            mailed_second,
        }
    }

    #[test]
    fn a_person_with_no_second_step_is_in_with_a_password_or_a_mailed_code_where_it_is_allowed() {
        let plain = can(false, false, false);
        assert!(plain.complete(&[Password]));
        assert!(!plain.complete(&[EmailCode]));
        assert!(!plain.complete(&[]));
        assert!(can(false, true, false).complete(&[EmailCode]));
        // Nothing is asked of them: a code that does not get them in is none.
        assert_eq!(plain.missing(&[EmailCode]), vec![]);
    }

    #[test]
    fn a_person_with_an_authenticator_needs_it_beside_the_password_or_a_mailed_code() {
        let with_app = can(true, true, false);
        assert!(!with_app.complete(&[Password]));
        assert!(with_app.complete(&[Password, Totp]));
        assert!(with_app.complete(&[EmailCode, Totp]));
        // An authenticator alone is never a first step.
        assert!(!with_app.complete(&[Totp]));
        assert_eq!(with_app.missing(&[Password]), vec![Totp]);
        assert_eq!(with_app.missing(&[EmailCode]), vec![Totp]);
    }

    #[test]
    fn a_mailed_code_as_the_only_second_step_asks_for_the_password_after_a_mailed_code() {
        let mail_only = can(false, true, true);
        assert_eq!(mail_only.missing(&[Password]), vec![EmailCode]);
        // One mailbox is one proof: a mailed code with nothing else lacks the password.
        assert!(!mail_only.complete(&[EmailCode]));
        assert_eq!(mail_only.missing(&[EmailCode]), vec![Password]);
        assert!(mail_only.complete(&[Password, EmailCode]));
    }

    #[test]
    fn an_unused_recovery_code_replaces_the_app_but_never_the_first_factor() {
        let mut app = can(true, true, false);
        app.recovery = true;
        assert!(app.complete(&[Password, SignInProof::RecoveryCode]));
        assert!(app.complete(&[EmailCode, SignInProof::RecoveryCode]));
        assert!(!app.complete(&[SignInProof::RecoveryCode]));
        assert!(!app.complete(&[Totp, SignInProof::RecoveryCode]));
        assert_eq!(
            app.missing(&[Password]),
            vec![Totp, SignInProof::RecoveryCode]
        );
        app.recovery = false;
        assert!(!app.complete(&[Password, SignInProof::RecoveryCode]));
    }

    #[test]
    fn a_person_with_both_second_steps_may_finish_with_either_in_the_order_of_the_contract() {
        let both = can(true, true, true);
        assert_eq!(both.missing(&[Password]), vec![Totp, EmailCode]);
        assert!(both.complete(&[Password, EmailCode]));
        assert!(both.complete(&[Password, Totp]));
        // A mailed code that is only allowed as a second step cannot begin a sign-in.
        let second_only = can(true, false, true);
        assert!(!second_only.complete(&[EmailCode, Totp]));
        assert_eq!(second_only.missing(&[EmailCode, Totp]), vec![Password]);
    }
}
