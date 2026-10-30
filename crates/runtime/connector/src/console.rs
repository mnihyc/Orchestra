//! The local console: a terminal interface to enrollment, environments, what the exec node offers, the tool servers found and imported, limits, providers and activity.
//!
//! Started in a terminal, `orch-connector` opens it full-screen: six screens, every choice a list or a switch,
//! each with a sentence of what it does. It reads what is in the data directory itself (who the machine is, the
//! owner's limits) and asks the running connector the rest over its local socket ([`crate::local_socket`]): the
//! session, the environments registered here, the tool servers found, what the system asked. It changes exactly
//! what the commands change: a switch of the limits and the directories environments may be made in are written
//! to the configuration file, which the running connector reads within seconds; a copy of a tool server is
//! forgotten by the connector. Keys: Tab and the arrows move, Enter switches, `a` adds, `d` removes, `f` forgets,
//! `r` reads again, `q` leaves.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use orch_link_contract::policy::PolicyKey;
use orch_types::Json;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tokio::runtime::Handle;

use crate::enrollment::DataDir;
use crate::local_socket::{ActivityRow, EnvironmentRow, McpRow, Reply, Request, Status, ask};
use crate::policy::PolicyStore;

pub mod enrollment;

/// The screens, in their order.
pub const SCREENS: [&str; 6] = [
    "Start",
    "Environments",
    "Tools",
    "Limits",
    "Providers",
    "Activity",
];
/// How often what the running connector says is read again.
const REFRESH: Duration = Duration::from_secs(2);
/// How long a key is waited for before the screen is drawn again.
const TICK: Duration = Duration::from_millis(250);

/// What a person is typing.
enum Input {
    /// A directory to offer environments in.
    Root(String),
    /// A system address or a complete copied enrollment command; never displayed verbatim.
    Enroll(String),
    /// Token entered separately after a system address.
    Token(enrollment::Parsed, String),
}

/// What the console shows.
struct State {
    dir: PathBuf,
    handle: Handle,
    version: String,
    named: bool,
    screen: usize,
    selected: ListState,
    input: Option<Input>,
    message: Option<String>,
    status: Result<Status, String>,
    environments: Vec<EnvironmentRow>,
    servers: Vec<McpRow>,
    activity: Vec<ActivityRow>,
    read_at: Option<Instant>,
}

impl State {
    fn new(dir: PathBuf, handle: Handle, version: String, named: bool) -> Self {
        let mut selected = ListState::default();
        selected.select(Some(0));
        Self {
            dir,
            handle,
            version,
            named,
            screen: 0,
            selected,
            input: None,
            message: None,
            status: Err("not read yet".to_owned()),
            environments: Vec::new(),
            servers: Vec::new(),
            activity: Vec::new(),
            read_at: None,
        }
    }

    fn ask(&self, request: &Request) -> Result<Reply, String> {
        self.handle
            .block_on(ask(&self.dir, request))
            .map_err(|error| error.to_string())
    }

    /// Reads again what the running connector says.
    fn refresh(&mut self) {
        self.read_at = Some(Instant::now());
        self.status = match self.ask(&Request::Status) {
            Ok(Reply::Status(status)) => Ok(status),
            Ok(other) => Err(format!("an unexpected answer: {other:?}")),
            Err(error) => Err(error),
        };
        if self.status.is_err() {
            return;
        }
        if let Ok(Reply::Environments { environments }) = self.ask(&Request::Environments) {
            self.environments = environments;
        }
        if let Ok(Reply::Mcp { servers }) = self.ask(&Request::McpList) {
            self.servers = servers;
        }
        if let Ok(Reply::Activity { rows }) = self.ask(&Request::Activity { limit: Some(100) }) {
            self.activity = rows;
        }
    }

    /// The owner's limits, as the configuration file says them.
    fn store(&self) -> Option<PolicyStore> {
        let data = DataDir::open(&self.dir).ok()?;
        let config = data.config(|_| None).ok()?;
        PolicyStore::connected(config, Some(data.config_path())).ok()
    }

    fn roots(&self) -> Vec<String> {
        self.store()
            .and_then(|store| store.policy())
            .map(|policy| policy.environment_roots)
            .unwrap_or_default()
    }

    /// The rows of the screen shown now; the first of them that can be chosen and changed are its own.
    fn rows(&self) -> Vec<String> {
        match self.screen {
            0 => self.start_rows(),
            1 => {
                let mut rows: Vec<String> = self
                    .roots()
                    .iter()
                    .map(|root| format!("directory    {root}"))
                    .collect();
                rows.extend(
                    self.environments
                        .iter()
                        .map(|environment| format!("environment  {}  {}", environment.root, environment.id)),
                );
                rows
            }
            2 => {
                let mut rows: Vec<String> = self
                    .servers
                    .iter()
                    .map(|server| {
                        format!(
                            "{:<22} {:<12} {:<6} {}{}",
                            server.name,
                            server.harness,
                            server.transport,
                            server.place,
                            if server.imported { "  imported" } else { "" }
                        )
                    })
                    .collect();
                if let Some(policy) = self.store().and_then(|store| store.policy()) {
                    rows.push(format!("exec.offers      {}", policy.get(PolicyKey::ExecOffers)));
                    rows.push(format!(
                        "agent.harnesses  {}",
                        policy.get(PolicyKey::AgentHarnesses)
                    ));
                }
                rows
            }
            3 => match self.store().and_then(|store| store.policy()) {
                Some(policy) => PolicyKey::ALL
                    .iter()
                    .map(|key| {
                        format!(
                            "{:<30} {:<40} {}{}",
                            key.as_str(),
                            policy.get(*key).to_string(),
                            if policy.delegates(*key) {
                                "handed to Orch"
                            } else {
                                "kept here"
                            },
                            if *key==PolicyKey::EnvironmentVnc {" · Includes this machine's served personal desktop."} else {""}
                        )
                    })
                    .collect(),
                None => vec!["The system made this machine: it has no limits of its own.".to_owned()],
            },
            4 => match self.store().and_then(|store| store.policy()) {
                Some(policy) => vec![
                    format!(
                        "provider.arrangements  {}",
                        policy.get(PolicyKey::ProviderArrangements)
                    ),
                    format!("provider.upstreams     {}", policy.get(PolicyKey::ProviderUpstreams)),
                    "A key is kept here, sealed to this machine and never sent: \
                     `orch-connector provider set-key --provider <id>` reads it from standard input."
                        .to_owned(),
                    "Signing in and out is done with the agent program itself, by its own login command."
                        .to_owned(),
                ],
                None => vec!["The system made this machine: its providers are the system's.".to_owned()],
            },
            _ => self
                .activity
                .iter()
                .map(|row| {
                    format!(
                        "{}  {:<26} {:<10} {}",
                        row.at,
                        row.kind,
                        row.outcome,
                        row.detail.clone().unwrap_or_default()
                    )
                })
                .collect(),
        }
    }

    fn start_rows(&self) -> Vec<String> {
        if !DataDir::enrolled_at(&self.dir) {
            return vec![
                format!("{} holds no identity: this machine is not enrolled.", self.dir.display()),
                "Enroll it with the command the Add machine dialog in Orch gives:".to_owned(),
                "  orch-connector enroll --url <address> --token <token>".to_owned(),
                "Press e, then paste the address and token or the whole Add machine command. The token stays hidden.".to_owned(),
            ];
        }
        let mut rows = Vec::new();
        if let Ok(data) = DataDir::open(&self.dir)
            && let Ok(record) = data.record()
        {
            rows.push(format!("Machine   {} ({})", record.name, record.machine_id));
            rows.push(format!(
                "System    {} ({})",
                record.system.name, record.system.public_url
            ));
        }
        match &self.status {
            Ok(status) => {
                rows.push(format!(
                    "Session   {}{}",
                    status.link,
                    status
                        .endpoint
                        .as_ref()
                        .map(|endpoint| format!(" through {endpoint}"))
                        .unwrap_or_default()
                ));
                rows.push(format!("Version   {}", status.version));
                rows.push(format!(
                    "Turns     {} running here now",
                    status.turns_running
                ));
                rows.push(format!(
                    "Automatic updates   {}",
                    if status.update_auto {
                        "on · checked hourly while idle"
                    } else {
                        "off"
                    }
                ));
                if let Some(recommended) = &status.recommended
                    && crate::update::newer(recommended, &status.version)
                {
                    rows.push(format!(
                        "Update    the system recommends {recommended}; {}",
                        if status.update_auto {
                            "it installs when this machine is idle"
                        } else {
                            "run orch-connector update to install it"
                        }
                    ));
                }
            }
            Err(error) => rows.push(format!("Session   {error}")),
        }
        rows
    }

    /// The sentence of what the screen's keys do.
    fn help(&self) -> &'static str {
        match self.screen {
            0 if !DataDir::enrolled_at(&self.dir) => {
                "e enrolls from a pasted address/token or the whole Add machine command; q leaves"
            }
            1 => {
                "a adds a directory environments may be made in (whoever reads a thread's files there reads all \
                  under it); d removes the one chosen"
            }
            2 => "i imports the chosen server as this machine's owner; f forgets its copy",
            3 => {
                "Enter switches the chosen limit on or off; a list is changed with `orch-connector policy set`"
            }
            _ => "Tab and the arrows move between screens and rows; r reads again; q leaves",
        }
    }

    fn act(&mut self, key: KeyCode) -> bool {
        if self.input.is_some() {
            match key {
                KeyCode::Char(character) => self.paste(&character.to_string()),
                KeyCode::Backspace => {
                    if let Some(input) = &mut self.input {
                        match input {
                            Input::Root(text) | Input::Enroll(text) | Input::Token(_, text) => {
                                text.pop();
                            }
                        }
                    }
                }
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => match self.input.take() {
                    Some(Input::Root(text)) => self.message = Some(self.add_root(&text)),
                    Some(Input::Enroll(text)) => match enrollment::parse(&text) {
                        Ok(parsed) if parsed.token.is_none() => {
                            self.input = Some(Input::Token(parsed, String::new()))
                        }
                        Ok(parsed) => self.message = Some(self.enroll(parsed)),
                        Err(error) => self.message = Some(error.to_string()),
                    },
                    Some(Input::Token(mut parsed, token)) => {
                        parsed.token = Some(token.trim().to_owned());
                        self.message = Some(self.enroll(parsed));
                    }
                    None => {}
                },
                _ => {}
            }
            return true;
        }
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Tab | KeyCode::Right => self.go(self.screen + 1),
            KeyCode::BackTab | KeyCode::Left => self.go(self.screen + SCREENS.len() - 1),
            KeyCode::Char(digit @ '1'..='6') => {
                self.go(usize::from(digit as u8 - b'1'));
            }
            KeyCode::Down => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Char('r') => {
                self.refresh();
                self.message = Some("Read again.".to_owned());
            }
            KeyCode::Char('a') if self.screen == 1 => self.input = Some(Input::Root(String::new())),
            KeyCode::Char('e') if self.screen == 0 && !DataDir::enrolled_at(&self.dir) => {
                self.input = Some(Input::Enroll(String::new()))
            }
            KeyCode::Char('d') if self.screen == 1 => self.message = Some(self.remove_root()),
            KeyCode::Char('f') if self.screen == 2 => self.message = Some(self.forget()),
            KeyCode::Char('i') if self.screen == 2 => self.message = Some(self.import()),
            KeyCode::Enter if self.screen == 3 => self.message = Some(self.switch()),
            _ => {}
        }
        true
    }

    fn paste(&mut self, pasted: &str) {
        if self.input.is_none() && self.screen == 0 && !DataDir::enrolled_at(&self.dir) {
            self.input = Some(Input::Enroll(String::new()));
        }
        if let Some(input) = &mut self.input {
            let text = match input {
                Input::Root(text) | Input::Enroll(text) | Input::Token(_, text) => text,
            };
            if text.len().saturating_add(pasted.len()) <= enrollment::MAX_TEXT_BYTES {
                text.push_str(pasted);
            } else {
                self.message = Some(
                    "The pasted text exceeds sixteen KiB; paste just the enrollment command."
                        .to_owned(),
                );
            }
        }
    }

    fn enroll(&self, parsed: enrollment::Parsed) -> String {
        use crate::enrollment::{EnrollInputs, HttpsEnroll, enroll_with_choices};
        use orch_link_contract::enrollment_choices::{
            CONNECTOR_NODE_REFUSAL, EnrollmentChoices, connector_roles,
        };
        use std::sync::Arc;
        if parsed.node {
            return CONNECTOR_NODE_REFUSAL.to_owned();
        }
        if let Some(named) = &parsed.data_dir {
            let chosen = std::path::absolute(named).ok();
            if chosen != std::path::absolute(&self.dir).ok() {
                return "The pasted command names another data directory. Open its console or paste only the address and token here.".to_owned();
            }
        }
        let result = (|| -> Result<String, String> {
            let data = DataDir::open(&self.dir).map_err(|error| error.to_string())?;
            let config = data
                .config(|name| std::env::var(name).ok())
                .map_err(|error| error.to_string())?;
            let token = parsed
                .token
                .as_deref()
                .filter(|token| !token.is_empty())
                .ok_or("Paste the enrollment token too.")?;
            let roles = connector_roles(
                parsed
                    .roles
                    .as_deref()
                    .unwrap_or(&config.connector.roles.join(",")),
            )
            .map_err(str::to_owned)?;
            let sections = if parsed.delegate_policy {
                vec!["*".into()]
            } else {
                parsed.delegate.as_ref().map_or_else(
                    || config.policy.delegated.clone(),
                    |value| value.split(',').map(str::to_owned).collect(),
                )
            };
            let choices = EnrollmentChoices::new(
                roles,
                sections,
                parsed.dedicated || config.connector.dedicated,
            )
            .map_err(str::to_owned)?;
            let clock: Arc<dyn orch_types::Clock> = Arc::new(orch_types::SystemClock);
            let inputs = EnrollInputs {
                address: parsed.address.clone(),
                token: token.to_owned(),
                name: parsed
                    .name
                    .clone()
                    .unwrap_or_else(|| crate::facts::Host::detect().hostname),
                roles: choices.roles.clone(),
                platform: crate::session::detect_platform(),
                connector_version: self.version.clone(),
                managed: false,
            };
            let enrolled = self
                .handle
                .block_on(enroll_with_choices(
                    &inputs,
                    &data,
                    &HttpsEnroll::new(clock.clone(), None),
                    &*clock,
                    &choices,
                ))
                .map_err(|error| error.to_string().replace(token, "[redacted]"))?;
            let said = format!(
                "Enrolled {} in {}.",
                enrolled.record.name, enrolled.record.system.name
            )
            .replace(token, "[redacted]");
            if parsed.no_service {
                return Ok(format!(
                    "{said} Start it with orch-connector --data-dir <this directory> run."
                ));
            }
            let program = std::env::current_exe().map_err(|error| error.to_string())?;
            match crate::service::act(
                crate::service::Action::Install,
                &crate::service::Service::new(program, self.dir.clone(), self.named),
            ) {
                Ok(_) => Ok(format!(
                    "{said} Its per-user service is installed and started."
                )),
                Err(error) => Ok(format!(
                    "{said} Its service could not be started: {error}. Use orch-connector --data-dir <this directory> run."
                )),
            }
        })();
        result.unwrap_or_else(|message| message)
    }

    fn footer(&self) -> Vec<String> {
        match &self.input {
            Some(Input::Root(text)) => vec![format!("Directory to offer: {text}_"),"Enter adds it, Esc leaves it".to_owned()],
            Some(Input::Enroll(text)) => vec![format!("Paste a system address or its enrollment command: {} characters entered (hidden)",text.chars().count()),
                "Enter continues; then enrolls and starts the per-user service (unless the command says --no-service). Esc cancels.".to_owned()],
            Some(Input::Token(_, token)) => vec![format!("Enrollment token: {} characters entered (hidden)",token.chars().count()),
                "Enter enrolls and starts the per-user service; Esc cancels.".to_owned()],
            None => vec![self.help().to_owned(),self.message.clone().unwrap_or_default()],
        }
    }

    fn go(&mut self, screen: usize) {
        self.screen = screen % SCREENS.len();
        self.selected.select(Some(0));
    }

    fn move_by(&mut self, step: isize) {
        let count = self.rows().len();
        if count == 0 {
            return;
        }
        let now = self.selected.selected().unwrap_or(0);
        let next = now.saturating_add_signed(step).min(count - 1);
        self.selected.select(Some(next));
    }

    fn chosen(&self) -> usize {
        self.selected.selected().unwrap_or(0)
    }

    fn set_roots(&self, roots: Vec<String>) -> String {
        let Some(store) = self.store() else {
            return "The system made this machine: it has no limits of its own.".to_owned();
        };
        match store.set_here(PolicyKey::EnvironmentRoots, &Json::from(roots)) {
            Ok(()) => "Written; the running connector takes it within seconds.".to_owned(),
            Err(error) => error.to_string(),
        }
    }

    fn add_root(&self, typed: &str) -> String {
        let absolute = match std::fs::canonicalize(typed.trim()) {
            Ok(path) if path.is_dir() => path.to_string_lossy().into_owned(),
            _ => return format!("{typed} is no directory here"),
        };
        let mut roots = self.roots();
        if roots.contains(&absolute) {
            return format!("{absolute} is offered already");
        }
        roots.push(absolute);
        self.set_roots(roots)
    }

    fn remove_root(&self) -> String {
        let mut roots = self.roots();
        let chosen = self.chosen();
        if chosen >= roots.len() {
            return "Choose one of the directories, the first rows.".to_owned();
        }
        roots.remove(chosen);
        self.set_roots(roots)
    }

    fn import(&mut self) -> String {
        let Some(server) = self.servers.get(self.chosen()).cloned() else {
            return "Choose a server.".to_owned();
        };
        let said = match self.ask(&Request::McpImport {
            name: server.name,
            from: Some(server.harness),
            place: Some(server.place),
        }) {
            Ok(Reply::Done { said }) => said,
            Ok(Reply::Error { message }) | Err(message) => message,
            Ok(other) => format!("an unexpected answer: {other:?}"),
        };
        self.refresh();
        said
    }

    fn forget(&mut self) -> String {
        let Some(server) = self.servers.get(self.chosen()).cloned() else {
            return "Choose a server.".to_owned();
        };
        if !server.imported {
            return format!("No copy of {} is kept here.", server.name);
        }
        let said = match self.ask(&Request::McpForget {
            name: server.name.clone(),
            from: Some(server.harness.clone()),
        }) {
            Ok(Reply::Done { said }) => said,
            Ok(Reply::Error { message }) | Err(message) => message,
            Ok(other) => format!("an unexpected answer: {other:?}"),
        };
        self.refresh();
        said
    }

    fn switch(&self) -> String {
        let Some(store) = self.store() else {
            return "The system made this machine: it has no limits of its own.".to_owned();
        };
        let Some(key) = PolicyKey::ALL.get(self.chosen()).copied() else {
            return "Choose a limit.".to_owned();
        };
        let Some(policy) = store.policy() else {
            return "The system made this machine: it has no limits of its own.".to_owned();
        };
        let Json::Bool(now) = policy.get(key) else {
            return format!(
                "{} is a list or a word: change it with `orch-connector policy set {} <value>`",
                key.as_str(),
                key.as_str()
            );
        };
        match store.set_here(key, &Json::Bool(!now)) {
            Ok(()) => format!(
                "{} is now {}; the running connector takes it within seconds.",
                key.as_str(),
                if now { "off" } else { "on" }
            ),
            Err(error) => error.to_string(),
        }
    }
}

fn draw(frame: &mut Frame, state: &mut State) {
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(frame.area());
    let tabs = Tabs::new(
        SCREENS
            .iter()
            .enumerate()
            .map(|(index, name)| format!("{} {name}", index + 1)),
    )
    .select(state.screen)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Orch connector "),
    )
    .highlight_style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(tabs, top);
    let items: Vec<ListItem> = state.rows().into_iter().map(ListItem::new).collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", SCREENS[state.screen])),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, body, &mut state.selected);
    let footer: Vec<Line<'_>> = state.footer().into_iter().map(Line::from).collect();
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: true }), bottom);
}

fn interact(terminal: &mut DefaultTerminal, state: &mut State) -> io::Result<()> {
    loop {
        if state.read_at.is_none_or(|at| at.elapsed() >= REFRESH) {
            state.refresh();
        }
        terminal.draw(|frame| draw(frame, state))?;
        if !event::poll(TICK)? {
            continue;
        }
        match event::read()? {
            Event::Paste(text) => state.paste(&text),
            Event::Key(key) if key.kind == KeyEventKind::Press && !state.act(key.code) => {
                return Ok(());
            }
            _ => {}
        }
    }
}

/// Opens the console on the data directory `dir` until the person leaves it.
pub async fn run(dir: PathBuf, version: String, named: bool) -> io::Result<()> {
    let handle = Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut terminal = ratatui::init();
        let mut state = State::new(dir, handle, version, named);
        let outcome = crossterm::execute!(io::stdout(), event::EnableBracketedPaste)
            .and_then(|()| interact(&mut terminal, &mut state));
        let _ = crossterm::execute!(io::stdout(), event::DisableBracketedPaste);
        ratatui::restore();
        outcome
    })
    .await
    .map_err(io::Error::other)?
}

/// Whether standard input and output are a terminal a person sits at, where `orch-connector` alone opens the
/// console.
#[must_use]
pub fn in_a_terminal() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A console on `dir`, with the runtime its requests would run on, kept alive beside it.
    fn state(dir: &Path) -> (tokio::runtime::Runtime, State) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state = State::new(
            dir.to_owned(),
            runtime.handle().clone(),
            "0.1.0".to_owned(),
            true,
        );
        (runtime, state)
    }

    #[test]
    fn the_keys_move_between_the_six_screens_and_rows_and_q_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let (_runtime, mut state) = state(dir.path());
        assert!(state.act(KeyCode::Tab));
        assert_eq!(state.screen, 1);
        assert!(state.act(KeyCode::BackTab));
        assert!(state.act(KeyCode::BackTab));
        assert_eq!(state.screen, SCREENS.len() - 1);
        assert!(state.act(KeyCode::Char('4')));
        assert_eq!(state.screen, 3);
        assert!(!state.act(KeyCode::Char('q')));
    }

    #[test]
    fn a_machine_that_is_not_enrolled_is_told_how_to_enroll() {
        let dir = tempfile::tempdir().unwrap();
        let (_runtime, state) = state(dir.path());
        let rows = state.start_rows();
        assert!(
            rows.iter().any(|row| row.contains("orch-connector enroll")),
            "{rows:?}"
        );
    }

    #[test]
    fn start_accepts_a_pasted_command_and_never_draws_its_token() {
        let dir = tempfile::tempdir().unwrap();
        let (_runtime, mut state) = state(dir.path());
        state.paste("orch-connector enroll --url https://orch.test --token oenr_private");
        assert!(matches!(&state.input, Some(Input::Enroll(_))));
        assert!(!state.footer().join(" ").contains("oenr_private"));
        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut state)).unwrap();
        let shown = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(shown.contains("hidden"));
        assert!(!shown.contains("oenr_private"));
        state.act(KeyCode::Esc);
        assert!(state.input.is_none());
    }
}
