//! Only an access token crosses the lease boundary; its renewal material stays in the native home.

use crate::{protocol::Rpc, provider};
use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::credentials::{GivenCredential, IssuedCredential};
use orch_plugin_contract::Secret;
use orch_types::{Clock, Json, Time};
use serde_json::json;
use std::path::Path;

pub(crate) const FORM: &str = "codex_access_token";

#[derive(Clone)]
pub(crate) struct Token {
    value: Secret,
    pub account: String,
    plan: Option<String>,
    pub expires: Time,
}

fn unusable() -> AdapterError {
    AdapterError::CredentialUnusable("A current account-bound access token is required.".into())
}

impl Token {
    fn read(value: Secret, now: Time) -> Result<Self, AdapterError> {
        let checked = provider::check_access_token(value.expose(), now).map_err(|_| unusable())?;
        let claims = provider::claims_of(value.expose()).ok_or_else(unusable)?;
        let account = claims
            .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
            .and_then(Json::as_str)
            .filter(|account| !account.is_empty())
            .ok_or_else(unusable)?
            .to_owned();
        let plan = claims
            .pointer("/https:~1~1api.openai.com~1auth/chatgpt_plan_type")
            .and_then(Json::as_str)
            .map(str::to_owned);
        Ok(Self {
            value,
            account,
            plan,
            expires: checked.expires_at.ok_or_else(unusable)?,
        })
    }
    pub fn given(value: GivenCredential, now: Time) -> Result<Self, AdapterError> {
        if value.form != FORM
            || value.upstream.is_some()
            || value.expires_at.is_some_and(|expiry| expiry <= now)
        {
            return Err(unusable());
        }
        let mut token = Self::read(value.credential, now)?;
        if let Some(expiry) = value.expires_at {
            token.expires = token.expires.min(expiry);
        }
        Ok(token)
    }
    pub fn response(&self) -> Json {
        json!({"accessToken":self.value.expose(),"chatgptAccountId":self.account,"chatgptPlanType":self.plan})
    }
    pub fn login(&self) -> Json {
        let mut value = self.response();
        value["type"] = json!("chatgptAuthTokens");
        value
    }
}

async fn current(home: &Path, clock: &dyn Clock) -> Result<Token, AdapterError> {
    #[derive(serde::Deserialize)]
    struct NativeTokens {
        access_token: String,
    }
    #[derive(serde::Deserialize)]
    struct NativeAuth {
        tokens: NativeTokens,
    }
    let path = home.join("auth.json");
    let token = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(path).map_err(|_| unusable())?;
        let auth: NativeAuth =
            serde_json::from_reader(std::io::BufReader::new(file)).map_err(|_| unusable())?;
        Ok::<_, AdapterError>(auth.tokens.access_token)
    })
    .await
    .map_err(|_| unusable())??;
    Token::read(Secret::new(token), clock.time())
}

pub(crate) async fn issue(
    rpc: &Rpc,
    home: &Path,
    clock: &dyn Clock,
    form: &str,
) -> Result<IssuedCredential, AdapterError> {
    if form != FORM {
        return Err(AdapterError::Unsupported(
            "this credential form is not supplied by the native sign-in".into(),
        ));
    }
    // The caller asks for a fresh copy, including proactive renewals before expiry.
    // The native keeper owns renewal; a still-valid token-only sign-in remains usable if it cannot renew.
    let _ = rpc
        .call_within(
            "account/read",
            json!({"refreshToken":true}),
            provider::ACCOUNT_TIMEOUT,
        )
        .await;
    let token = current(home, clock).await?;
    Ok(IssuedCredential {
        form: FORM.into(),
        credential: token.value,
        expires_at: Some(token.expires),
    })
}
