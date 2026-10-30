//! Unit and directory roles use the same command journal, with each machine's policy and actual runtime context.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod rig_environment;

use common::Rig;
use orch_connector_core::roles::provider::{ProviderParts, plug_in};
use orch_link_contract::{
    commands::{AgentStart, EnvAttach, IndexMode, Secret, SerialKey, UnitAfterUp},
    session::Role,
};
use orch_settings_contract::unit::AfterUpCommand;
use orch_types::new_id;
use rig_environment::EnvRig;
use std::{collections::BTreeMap, sync::Arc};

#[tokio::test]
async fn a_managed_environment_uses_the_workspace_and_attached_variables_and_secrets_and_replays_after_connector_restart()
 {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("unit-workspace");
    rig.tools_dir = Some(rig.directory("managed-tools"));
    rig.start().await;
    let attach = new_id();
    let secret = "unit-secret-981";
    let attached = rig
        .send_as(
            attach,
            &EnvAttach {
                resources: None,
                data_generation: None,
                data_sources: None,
                storage: None,
                environment_id: rig.environment,
                workspace_root: Some(workspace.display().to_string()),
                ignore: Vec::new(),
                index: IndexMode::Full,
                mounts: Some(Vec::new()),
                variables: BTreeMap::from([("AFTER_UP_VALUE".into(), "a b;$HOME".into())]),
                secrets: BTreeMap::from([(
                    "AFTER_UP_SECRET".into(),
                    Secret::Sealed(rig.sealed(attach, secret)),
                )]),
            },
        )
        .await;
    assert!(matches!(
        attached,
        orch_link::commands::Progress::Succeeded(_)
    ));
    let body=UnitAfterUp{run_id:new_id(),environment_id:Some(rig.environment),provider_id:None,agent_id:None,position:0,name:"10-prepare".into(),
        run:AfterUpCommand{command:vec!["/bin/sh".into(),"-c".into(),"printf '%s\\n' \"$AFTER_UP_VALUE\" >> prepared; printf '%s' \"$AFTER_UP_SECRET\" > received; printf '%s' \"$AFTER_UP_SECRET\"".into()],timeout_secs:30}};
    let first = rig.succeed(&body).await;
    assert_eq!(first["exit"], 0);
    assert!(!first.to_string().contains(secret));
    assert_eq!(
        std::fs::read_to_string(workspace.join("prepared")).unwrap(),
        "a b;$HOME\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("received")).unwrap(),
        secret
    );
    rig.stop().await;
    rig.start().await;
    assert_eq!(rig.succeed(&body).await, first);
    assert_eq!(
        std::fs::read_to_string(workspace.join("prepared"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let next = UnitAfterUp {
        run_id: new_id(),
        ..body
    };
    assert_eq!(rig.succeed(&next).await["exit"], 0);
    assert_eq!(
        std::fs::read_to_string(workspace.join("prepared"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_provider_role_runs_in_its_home_before_provider_start_and_does_not_claim_an_environment_command()
 {
    let mut rig = Rig::with_roles(vec![Role::Provider]);
    rig.managed = true;
    rig.plug = Some(Arc::new(|builder, roles, data, key, clock| {
        plug_in(
            builder,
            roles,
            ProviderParts::new(common::model::protocols(), data, key, clock).unwrap(),
        )
    }));
    rig.start().await;
    let files = tempfile::tempdir().unwrap();
    let output = files.path().join("runs");
    let body=UnitAfterUp{run_id:new_id(),environment_id:None,provider_id:Some(rig.provider),agent_id:None,position:0,name:"10-provider-tool".into(),
        run:AfterUpCommand{command:vec!["/bin/sh".into(),"-c".into(),"test \"$PWD\" = \"$HOME\" && test \"${CARGO_MANIFEST_DIR-}\" = \"$3\" && printf '%s\\n' \"$1\" >> \"$2\"".into(),"helper".into(),"literal a b;$HOME".into(),output.display().to_string(),std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()],timeout_secs:30}};
    let key = SerialKey::Runtime(new_id());
    let first = rig.succeed(&key, &body).await;
    assert_eq!(first["exit"], 0);
    rig.kill().await;
    rig.start().await;
    assert_eq!(rig.succeed(&key, &body).await, first);
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "literal a b;$HOME\n"
    );
    let wrong = UnitAfterUp {
        run_id: new_id(),
        provider_id: None,
        environment_id: Some(new_id()),
        ..body
    };
    assert_eq!(rig.fail(&key, &wrong).await, "not_found");
    rig.kill().await;
}

#[tokio::test]
async fn a_directory_keeps_commands_off_as_installed_and_can_enable_the_same_commands_locally() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("personal-workspace");
    rig.start().await;
    rig.attach(&workspace).await;
    let body = UnitAfterUp {
        run_id: new_id(),
        environment_id: Some(rig.environment),
        provider_id: None,
        agent_id: None,
        position: 0,
        name: "10-refused".into(),
        run: AfterUpCommand {
            command: vec!["/bin/sh".into(), "-c".into(), "touch must-not-exist".into()],
            timeout_secs: 30,
        },
    };
    assert_eq!(rig.fail(&body).await, "unsupported");
    assert!(!workspace.join("must-not-exist").exists());
    rig.connector
        .as_ref()
        .unwrap()
        .services()
        .policy
        .set_here(
            orch_link_contract::policy::PolicyKey::EnvironmentAfterUp,
            &serde_json::json!(true),
        )
        .unwrap();
    assert_eq!(rig.succeed(&body).await["exit"], 0);
    assert!(workspace.join("must-not-exist").exists());
    rig.stop().await;
}

#[tokio::test]
async fn directory_provider_commands_use_the_selected_profile_home_and_runtime_variables() {
    let mut rig = Rig::with_roles(vec![Role::Agent, Role::Environment]);
    rig.config.environment.after_up = true;
    rig.config.policy.local_keys = Some(vec!["environment.after_up".into()]);
    rig.start().await;
    let start:AgentStart=serde_json::from_value(serde_json::json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake",
        "profile":{"env":{"AFTER_UP_VALUE":"profile"}},"home":null,"provider_config":{},"runtime":{"variables":{"AFTER_UP_VALUE":"runtime value"}}})).unwrap();
    rig.succeed(&rig.harness_key(), &start).await;
    let body = UnitAfterUp {
        run_id: new_id(),
        environment_id: None,
        provider_id: Some(rig.provider),
        agent_id: Some(rig.agent),
        position: 0,
        name: "prepare".into(),
        run: AfterUpCommand {
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf '%s' \"$AFTER_UP_VALUE\" >> prepared".into(),
            ],
            timeout_secs: 30,
        },
    };
    let key = SerialKey::Runtime(rig.provider);
    assert_eq!(rig.succeed(&key, &body).await["exit"], 0);
    let home = rig
        .data
        .path()
        .join("harness")
        .join(rig.provider.to_string())
        .join(rig.agent.to_string());
    assert_eq!(
        std::fs::read_to_string(home.join("prepared")).unwrap(),
        "runtime value"
    );
    rig.succeed(&key, &body).await;
    assert_eq!(
        std::fs::read_to_string(home.join("prepared")).unwrap(),
        "runtime value",
        "replay does not run it twice"
    );
    rig.kill().await;
}
