//! The Google CLI performs OAuth on the provider's machine. Only its sign-in link
//! and the user's single-use code cross the existing interactive login channel.
use super::{Shared, antigravity, process::LaunchRequest};
use orch_agent_contract::adapter::{AdapterError, AdapterEvent};
use orch_agent_contract::login::{LoginShow, LoginState, LoginStateKind, LoginValues, ShowKind};
use orch_types::{Field, FieldType};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

pub(super) struct Flow {
    ready: Arc<AtomicBool>,
    input: Option<oneshot::Sender<String>>,
    cancel: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

fn google_link(word: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(word).ok()?;
    let start = text.find("https://accounts.google.com/")?;
    let url = reqwest::Url::parse(&text[start..]).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("accounts.google.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "/o/oauth2/auth" | "/o/oauth2/v2/auth")
    {
        return None;
    }
    let field = |key: &str| {
        let mut values = url
            .query_pairs()
            .filter(|(name, _)| name == key)
            .map(|(_, v)| v.into_owned());
        let value = values.next()?;
        values.next().is_none().then_some(value)
    };
    if field("response_type").as_deref() != Some("code")
        || field("redirect_uri").as_deref() != Some("https://antigravity.google/oauth-callback")
        || field("state").is_none_or(|v| v.is_empty())
    {
        return None;
    }
    Some(url.to_string())
}

// Drain both streams even after exposing the link. No other terminal output is
// forwarded or logged, including output after the code has been submitted.
async fn read_link(mut pipe: impl AsyncRead + Unpin, links: mpsc::Sender<String>) {
    let mut word = Vec::new();
    let mut buf = [0_u8; 1024];
    let mut skip = false;
    while let Ok(n) = pipe.read(&mut buf).await {
        for &byte in &buf[..n] {
            if byte.is_ascii_control() || byte.is_ascii_whitespace() {
                if !skip && let Some(link) = google_link(&word) {
                    let _ = links.try_send(link);
                }
                word.clear();
                skip = false;
            } else if !skip {
                // Bound memory if unrelated CLI diagnostics contain an unbroken line.
                if word.len() == 16 * 1024 {
                    word.clear();
                    skip = true;
                } else {
                    word.push(byte);
                }
            }
        }
        if n == 0 {
            if !skip && let Some(link) = google_link(&word) {
                let _ = links.try_send(link);
            }
            break;
        }
    }
}

impl Flow {
    pub(super) async fn start(shared: Arc<Shared>) -> Result<Self, AdapterError> {
        let mut plan = antigravity::plan(&shared)?;
        plan.args.push("--login".into());
        let mut child = shared
            .launcher
            .launch(LaunchRequest {
                program: shared.binary.clone(),
                args: plan.args,
                env: plan.env,
                cwd: Some(shared.home_root.join("probe/neutral")),
            })
            .await
            .map_err(|_| {
                AdapterError::HarnessUnavailable(
                    "Could not start Google Antigravity sign-in.".into(),
                )
            })?;
        let (tx, mut code) = oneshot::channel::<String>();
        let (cancel, mut cancelled) = oneshot::channel();
        let ready = Arc::new(AtomicBool::new(false));
        let prompted = Arc::clone(&ready);
        let task = tokio::spawn(async move {
            let sink = &shared.env.sink;
            let _ = sink
                .emit(AdapterEvent::Login(LoginState::starting(
                    antigravity::METHOD,
                )))
                .await;
            let (links, mut urls) = mpsc::channel(1);
            let mut readers = JoinSet::new();
            readers.spawn(read_link(child.stdout, links.clone()));
            if let Some(pipe) = child.stderr.take() {
                readers.spawn(read_link(pipe, links.clone()));
            }
            drop(links);
            let mut sent = false;
            let mut waiting = true;
            let flow = async {
                loop {
                    tokio::select! {
                        exit=child.control.wait()=> break exit.code==Some(0),
                        link=urls.recv(), if waiting => {
                            let Some(value)=link else {waiting=false;continue};
                            waiting=false;
                            prompted.store(true,Ordering::Release);
                            let mut state=LoginState::starting(antigravity::METHOD);
                            state.state=LoginStateKind::AwaitingUser;
                            state.shows.push(LoginShow{kind:ShowKind::Uri,label:"Open Google sign-in".into(),value});
                            state.asks=Some(vec![Field::new("code","Authorization code",FieldType::Secret)]);
                            if sink.emit(AdapterEvent::Login(state)).await.is_err(){break false;}
                        }
                        answer=&mut code, if !sent=>{
                            let Ok(answer)=answer else {break false};
                            sent=true;
                            if child.stdin.write_all(answer.as_bytes()).await.is_err() || child.stdin.write_all(b"\n").await.is_err() || child.stdin.shutdown().await.is_err(){break false;}
                        }
                    }
                }
            };
            let finished = tokio::select! {
                biased;
                _=&mut cancelled=>None,
                result=tokio::time::timeout(std::time::Duration::from_secs(900),flow)=> Some(result.unwrap_or(false)),
            };
            prompted.store(false, Ordering::Release);
            child.control.kill();
            let Some(ok) = finished else { return };
            let verified = ok
                && tokio::select! {
                    biased; _=&mut cancelled=>return,
                    result=antigravity::verified_models(&shared)=>result.is_ok()
                };
            let state = if verified {
                LoginState::completed(antigravity::METHOD)
            } else {
                LoginState::failed(
                    antigravity::METHOD,
                    "Google sign-in did not complete or the CLI could not open an authenticated session.",
                )
            };
            let _ = sink.emit(AdapterEvent::Login(state)).await;
        });
        Ok(Self {
            ready,
            input: Some(tx),
            cancel: Some(cancel),
            task,
        })
    }

    pub(super) fn answer(&mut self, values: LoginValues) -> Result<(), AdapterError> {
        let code = values.text("code").unwrap_or_default().trim();
        if code.is_empty()
            || code.contains(char::is_whitespace)
            || code.chars().any(char::is_control)
            || code.contains("://")
        {
            return Err(AdapterError::CredentialUnusable(
                "Paste the authorization code from the Google page.".into(),
            ));
        }
        if !self.ready.load(Ordering::Acquire) {
            return Err(AdapterError::NotFound(
                "No Google sign-in is waiting for a code.".into(),
            ));
        }
        self.input
            .take()
            .ok_or_else(|| AdapterError::NotFound("The Google code was already submitted.".into()))?
            .send(code.to_owned())
            .map_err(|_| AdapterError::NotFound("Google sign-in ended.".into()))
    }

    pub(super) async fn cancel(mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        let _ = self.task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::google_link;

    #[test]
    fn a_sign_in_link_only_opens_the_google_authorization_page_for_the_cli_callback() {
        let url = "https://accounts.google.com/o/oauth2/auth?response_type=code&state=example&redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback";
        assert!(google_link(url.as_bytes()).is_some());
        for other in [
            url.replace("accounts.google.com/", "accounts.google.com.invalid/"),
            url.replace(
                "https%3A%2F%2Fantigravity.google",
                "https%3A%2F%2Fexample.org",
            ),
            url.replace("state=example", "state="),
            format!("{url}&state=other"),
        ] {
            assert!(google_link(other.as_bytes()).is_none());
        }
    }
}
