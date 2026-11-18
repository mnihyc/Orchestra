//! Keep first-start registration alive until the installed node redeems its token.
use super::{Adopted, CertificateService, EnrollmentToken, LinkFault, raw_hash, token_digest};
use orch_api_contract::admin::BundledNodeEnrollment as Status;
use std::{path::Path, sync::PoisonError, time::Duration};
use tokio_util::sync::CancellationToken;

// Same cadence as the installed node's first enrollment request.
const RETRY: Duration = Duration::from_secs(10);

pub(super) enum FileError {
    Unreadable,
    Invalid,
    Authority,
}

impl FileError {
    fn status(&self) -> Status {
        match self {
            Self::Unreadable => Status::TokenFileUnreadable,
            Self::Invalid => Status::InvalidTokenFile,
            Self::Authority => Status::DifferentAuthority,
        }
    }
    pub(super) fn fault(&self) -> LinkFault {
        match self {
            Self::Unreadable => LinkFault::new(
                500,
                "internal",
                "the bundled host token file could not be read",
            ),
            Self::Invalid => LinkFault::invalid("the token file does not hold a token"),
            Self::Authority => {
                LinkFault::invalid("the token file was made under another authority")
            }
        }
    }
}

pub(super) async fn read_file(
    path: &Path,
    service: &CertificateService,
) -> Result<EnrollmentToken, FileError> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|_| FileError::Unreadable)?;
    let token: EnrollmentToken = text.trim().parse().map_err(|_| FileError::Invalid)?;
    if !token.pins(&service.authority().pin()) {
        return Err(FileError::Authority);
    }
    Ok(token)
}

impl CertificateService {
    /// Current progress for the local bootstrap task, without file contents or database details.
    pub fn bundled_node_enrollment(&self) -> Option<Status> {
        *self
            .inner
            .bundled_registration
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Coalesce retries on the single task that owns registration of the installation token.
    pub fn retry_bundled_enrollment(&self) {
        self.inner.bundled_retry.notify_one();
    }

    /// Retry transient failures and reread a corrected file without restarting either service.
    pub async fn register_bundled_node(&self, path: &Path, stop: CancellationToken) {
        let mut admitted: Option<Vec<u8>> = None;
        loop {
            let attempt = async {
                if let Some(hash) = &admitted {
                    let mut conn = self.inner.db.acquire().await.map_err(LinkFault::from)?;
                    let row =
                        orch_db::queries::resources::enrollment_token(&mut conn, hash).await?;
                    let Some(row) = row.filter(|r| r.used_at.is_none()) else {
                        return Ok(None);
                    };
                    let now = self.inner.clock.now();
                    let lifetime = chrono::Duration::seconds(super::BOOTSTRAP_TOKEN_SECS as i64);
                    if row.expires_at < now + lifetime / 2 {
                        orch_db::queries::resources::extend_unused_token(
                            &mut conn,
                            hash,
                            now + lifetime,
                        )
                        .await?;
                    }
                    return Ok(Some(Status::WaitingForNode));
                }
                let token = match read_file(path, self).await {
                    Ok(token) => token,
                    Err(error) => return Ok(Some(error.status())),
                };
                match self.adopt_bundled_token(&token).await? {
                    Adopted::WaitingForAdministrator => Ok(Some(Status::WaitingForAdministrator)),
                    Adopted::Registered | Adopted::AlreadyKnown => {
                        admitted = Some(raw_hash(&token_digest(token.secret())));
                        Ok(Some(Status::WaitingForNode))
                    }
                }
            };
            let status = tokio::select! {
                () = stop.cancelled() => return,
                outcome = attempt => outcome.unwrap_or_else(|_: LinkFault| Some(Status::RegistrationUnavailable)),
            };
            {
                let mut previous = self
                    .inner
                    .bundled_registration
                    .write()
                    .unwrap_or_else(PoisonError::into_inner);
                if *previous != status {
                    tracing::info!(?status, "bundled host enrollment changed");
                    *previous = status;
                }
            }
            if status.is_none() {
                return;
            }
            tokio::select! {
                () = stop.cancelled() => return,
                () = self.inner.bundled_retry.notified() => {},
                () = tokio::time::sleep(RETRY) => {},
            }
        }
    }
}
