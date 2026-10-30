//! Compose connector roles, adapters, executors, model protocols and environment host.
//!
//! The program that runs on a machine so that Orch can use it. `enroll` joins a system with a token from
//! it; `run` holds the session and does what the system asks within the limits of the machine's owner;
//! `status`, `policy` and `env` show and set those limits; `provider set-key` keeps a key on the machine,
//! which never sends it; `service` installs and controls the per-user service that keeps it running;
//! `doctor` checks the identity, the gateway's addresses, the clock, the harnesses and the log; `update` puts
//! the system's newer build in this program's place; `mcp` asks the running connector which tool servers its
//! harnesses have configured; `console`, or the program alone in a terminal, opens the console. `proxy`, which nobody types, is what a host's node runs
//! beside a restricted unit as its only way out.

#![forbid(unsafe_code)]

mod arguments;
mod composition;
mod enrollment_options;
mod local;
mod mcp;
mod ports;
mod programs;
mod uninstall;
mod update;

use std::process::ExitCode;

use anyhow::Context;
use arguments::{Args, Command, ForwardTransport, Port, PortForward, Provider};
use clap::Parser;
use orch_build_info::BuildInfo;
use std::sync::Arc;

const BUILD: BuildInfo = BuildInfo::new(
    "connector",
    env!("ORCH_RELEASE_VERSION"),
    env!("ORCH_RELEASE_TAG"),
    env!("ORCH_CONNECTOR_BUILD_NUMBER"),
);

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> anyhow::Result<()> {
    // Alone, in a terminal a person sits at, it is the console; started by a service, it runs.
    let command = args.command.unwrap_or_else(|| {
        if orch_connector_core::console::in_a_terminal() {
            Command::Console
        } else {
            Command::Run(arguments::Run { managed: false })
        }
    });
    match command {
        Command::Uninstall(options) => uninstall::run(args.data_dir, &options).await,
        Command::RecordInstallation(options) => uninstall::record(args.data_dir, &options),
        Command::Run(options) => composition::run(args.data_dir, options.managed, BUILD).await,
        #[cfg(unix)]
        Command::Proxy(options) => {
            orch_driver_docker::egress::program::run(
                orch_driver_docker::egress::program::Options {
                    dir: options.dir,
                    listen: options.listen,
                    host_inventory: options.host_inventory,
                },
                std::sync::Arc::new(orch_types::SystemClock),
            )
            .await?;
            Ok(())
        }
        Command::Enroll(options) => {
            let dir = composition::data_dir_of(args.data_dir);
            let said = composition::enroll_machine(&dir, &options, BUILD).await?;
            println!("{said}");
            Ok(())
        }
        Command::Dedicated { value } => {
            println!(
                "{}",
                local::dedicated(&composition::data_dir_of(args.data_dir), value)?
            );
            Ok(())
        }
        Command::Status => {
            let dir = composition::data_dir_of(args.data_dir);
            println!(
                "{}",
                local::status(
                    &dir,
                    &composition::adapter_kinds()?
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                )?
            );
            Ok(())
        }
        Command::Policy(command) => {
            let dir = composition::data_dir_of(args.data_dir);
            println!("{}", local::policy(&dir, &command)?);
            Ok(())
        }
        Command::Roles(command) => {
            println!(
                "{}",
                local::roles(&composition::data_dir_of(args.data_dir), &command)?
            );
            Ok(())
        }
        Command::Provider(Provider::SetKey { provider }) => {
            let dir = composition::data_dir_of(args.data_dir);
            println!(
                "{}",
                local::set_key(&dir, &provider, std::io::stdin().lock())?
            );
            Ok(())
        }
        Command::Port(Port::Forward(options)) => port_forward(options).await,
        Command::Ports(command) => ports::run(command).await,
        Command::Version => {
            println!("{BUILD}");
            println!("adapters: {}", composition::adapter_kinds()?.join(", "));
            Ok(())
        }
        Command::Env(command) => {
            let dir = composition::data_dir_of(args.data_dir);
            println!("{}", local::env_roots(&dir, &command)?);
            Ok(())
        }
        Command::Service(command) => {
            use orch_connector_core::service::{self, Action};
            // A named directory has a service of its own, as the installers name it.
            let named = args.data_dir.is_some();
            let dir = composition::data_dir_of(args.data_dir);
            let mut environment = Vec::new();
            let action = match command {
                arguments::Service::Install(install) => {
                    anyhow::ensure!(
                        orch_connector_core::enrollment::DataDir::enrolled_at(&dir),
                        "{} holds no identity: enroll first, with `orch-connector enroll --url <address> --token <token>`",
                        dir.display()
                    );
                    // A Windows task starts with the person's own variables, so the shell's proxy is already its own.
                    if cfg!(target_os = "windows") {
                        anyhow::ensure!(
                            install.proxy.is_none() && install.no_proxy.is_none(),
                            "on Windows the connector's task starts with your account's variables: set HTTPS_PROXY \
                             (and NO_PROXY) with `setx`, then install the service again"
                        );
                    } else {
                        environment = service::proxy_environment(
                            |name| std::env::var(name).ok(),
                            install.proxy.as_deref(),
                            install.no_proxy.as_deref(),
                        )?;
                    }
                    Action::Install
                }
                arguments::Service::Uninstall => Action::Uninstall,
                arguments::Service::Start => Action::Start,
                arguments::Service::Stop => Action::Stop,
                arguments::Service::Status => Action::Status,
            };
            let program = std::env::current_exe().context("finding this program's file")?;
            if matches!(action, Action::Install) {
                orch_connector_core::uninstall::register(
                    &program,
                    &dir,
                    Vec::new(),
                    named,
                    true,
                    false,
                )?;
            }
            println!(
                "{}",
                service::act(
                    action,
                    &service::Service::new(program, dir, named).with_environment(environment)
                )?
            );
            Ok(())
        }
        Command::Doctor => {
            use orch_connector_core::doctor::{Level, examine};
            let dir = composition::data_dir_of(args.data_dir);
            let clock: Arc<dyn orch_types::Clock> = Arc::new(orch_types::SystemClock);
            let lines = examine(&dir, |name| std::env::var(name).ok(), clock).await;
            for line in &lines {
                println!("{}", line.text());
            }
            let failed = lines
                .iter()
                .filter(|line| line.level == Level::Fail)
                .count();
            anyhow::ensure!(failed == 0, "{failed} of the checks failed");
            Ok(())
        }
        Command::Update => {
            let named = args.data_dir.is_some();
            let dir = composition::data_dir_of(args.data_dir);
            println!("{}", update::update(&dir, named, BUILD).await?);
            Ok(())
        }
        Command::Mcp(command) => {
            let dir = composition::data_dir_of(args.data_dir);
            println!("{}", mcp::mcp(&dir, &command).await?);
            Ok(())
        }
        Command::Console => {
            let named = args.data_dir.is_some();
            let dir = composition::data_dir_of(args.data_dir);
            orch_connector_core::console::run(dir, BUILD.version.to_owned(), named)
                .await
                .context("the console")?;
            Ok(())
        }
    }
}

/// `port forward`: reads the person's token or the whole link from standard input, listens on this
/// machine's loopback and carries each connection or sender until it is stopped. The credential is
/// never printed.
async fn port_forward(options: PortForward) -> anyhow::Result<()> {
    use orch_connector_core::port_forward::{self, Credential, Forward, Transport};
    let mut secret = String::new();
    std::io::stdin()
        .read_line(&mut secret)
        .context("reading standard input")?;
    let secret = secret.trim().to_owned();
    anyhow::ensure!(
        !secret.is_empty(),
        "standard input held nothing: give the token, or the whole link, there"
    );
    let (transport, word) = match options.transport {
        ForwardTransport::Tcp => (Transport::Tcp, "TCP"),
        ForwardTransport::Udp => (Transport::Udp, "UDP"),
    };
    let (base, credential, target) = if options.link_stdin {
        (
            Forward::base_of_link(&secret)?,
            Credential::Link(secret),
            "the port of the link".to_owned(),
        )
    } else {
        let url = options.url.context("--url names the installation")?;
        let environment = options
            .environment
            .context("--environment names the environment")?;
        let port = options.port.context("--port names the port")?;
        (
            orch_connector_core::https::https_base(&url)?,
            Credential::Token {
                token: secret,
                environment,
                port,
            },
            format!("port {port} of environment {environment}"),
        )
    };
    let listener = port_forward::bind(options.listen, transport).await?;
    let local = listener.local_addr().context("the address listened on")?;
    println!("Carrying {word} on {local} to {target}. Stop it with Ctrl-C.");
    port_forward::carry(
        Arc::new(Forward {
            base,
            credential,
            transport,
        }),
        listener,
        async {
            let _ = tokio::signal::ctrl_c().await;
        },
    )
    .await;
    Ok(())
}
