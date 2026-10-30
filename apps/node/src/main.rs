//! Compose the host runtime service with configured environment drivers.
//!
//! The program that lets the system use a host: `enroll` joins a system with a host token, `run` loads the
//! drivers the configuration names and does what the system asks of the host (make an image available, make a
//! volume, put a tool in the cache, make, start, stop and remove the units of managed runtimes), and `status`
//! shows who the host is. What is concrete about it is in the library of this package.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;
use orch_build_info::BuildInfo;
use orch_node::arguments::{Args, Command, Enrollment, Ports};
use orch_node::composition::{self, token_of};

const BUILD: BuildInfo = BuildInfo::new(
    "node",
    env!("ORCH_RELEASE_VERSION"),
    env!("ORCH_RELEASE_TAG"),
    env!("ORCH_SERVER_BUILD_NUMBER"),
);

fn main() -> ExitCode {
    let args = Args::parse();
    // Namespace entry happens before the node creates any worker thread.
    let outcome = if let Some(Command::RuntimeHelper(helper)) = &args.command {
        orch_node::runtime_helper::run(helper)
    } else {
        match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(run(args)),
            Err(error) => Err(error.into()),
        }
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn enrollment_of(arguments: &Enrollment) -> anyhow::Result<composition::Enrollment> {
    Ok(composition::Enrollment {
        delegate_policy: arguments.delegate_policy,
        url: arguments.url.clone(),
        token: token_of(arguments.token.as_deref(), arguments.token_file.as_deref())?,
        name: arguments.name.clone(),
    })
}

async fn run(args: Args) -> anyhow::Result<()> {
    let command = args.command.unwrap_or(Command::Run(Enrollment {
        delegate_policy: false,
        url: None,
        token: None,
        token_file: None,
        name: None,
    }));
    match command {
        Command::Uninstall(options) => {
            orch_node::uninstall::run(&args.config, args.data_dir, &options).await
        }
        Command::RecordInstallation(options) => {
            orch_node::uninstall::record(&args.config, args.data_dir, &options)
        }
        Command::HarnessConformance(args) => orch_node::conformance::run(&args).await,
        Command::Run(enrollment) => {
            composition::run(
                &args.config,
                args.data_dir,
                &enrollment_of(&enrollment)?,
                BUILD,
            )
            .await
        }
        Command::Enroll(enrollment) => {
            let said = composition::enroll(
                &args.config,
                args.data_dir,
                &enrollment_of(&enrollment)?,
                BUILD,
            )
            .await?;
            println!("{said}\n`orch-node run` holds the session.");
            Ok(())
        }
        Command::Status => {
            println!("{}", composition::status(&args.config, args.data_dir)?);
            Ok(())
        }
        Command::Update => {
            println!(
                "{}",
                orch_node::update::update(&args.config, args.data_dir, BUILD.version).await?
            );
            Ok(())
        }
        Command::Policy(command) => {
            println!(
                "{}",
                composition::policy(&args.config, args.data_dir, &command)?
            );
            Ok(())
        }
        Command::Ports(Ports::Show) => {
            println!("{}", composition::ports_show(&args.config)?);
            Ok(())
        }
        Command::Ports(Ports::Set(set)) => {
            println!("{}", composition::ports_set(&args.config, &set)?);
            Ok(())
        }
        Command::Version => {
            println!("{BUILD}");
            Ok(())
        }
        Command::RuntimeHelper(_) => {
            anyhow::bail!("the runtime helper must start before the node runtime")
        }
    }
}
