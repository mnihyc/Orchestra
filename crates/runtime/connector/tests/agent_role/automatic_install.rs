//! The real install supervisor must distinguish a retained automatic grant from an explicit one.
use super::common::{PATIENCE, Rig};
use orch_connector_core::{
    automatic,
    enrollment::{DataDir, EnrollmentRecord},
    update::{Offer, UpdateError},
};
use orch_link::machine::LinkStatus;
use orch_link_contract::{
    enrollment::EnrollReplySystem,
    session::{Role, UpdatePermit},
    version::{ConnectorVersions, RejectCode},
};
use orch_testkit::doubles::{controller::ScriptedController, link::Hub};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct Install {
    prepared: AtomicUsize,
    installed: AtomicUsize,
}
#[async_trait::async_trait]
impl automatic::Updates for Install {
    fn running_version(&self) -> &str {
        "0.1.0"
    }
    async fn prepare(&self) -> Result<Option<automatic::Prepared>, UpdateError> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("verified-build");
        std::fs::write(&path, b"verified fixture").unwrap();
        Ok(Some(automatic::Prepared::new(
            Offer {
                version: "2.0.0".into(),
                download_path: "/fixture".into(),
                size: 16,
                sha256: "0".repeat(64),
            },
            path,
            directory,
        )))
    }
    fn install(&self, _: &automatic::Prepared) -> Result<(), UpdateError> {
        self.installed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn physical_data(rig: &Rig) -> DataDir {
    let data = DataDir::open(rig.data.path()).unwrap();
    std::fs::create_dir_all(data.identity_dir()).unwrap();
    std::fs::write(
        data.identity_dir().join("enrollment.json"),
        serde_json::to_vec(&EnrollmentRecord {
            gateway_measured_at: None,
            gateways: Vec::new(),
            gateway_measurements: Vec::new(),
            server_endpoints: Vec::new(),
            machine_id: rig.machine_id,
            name: "local".into(),
            system: EnrollReplySystem {
                name: "test".into(),
                public_url: "https://orch.test".into(),
            },
            pin: "test".into(),
            protocol_version: 1,
            enrolled_at: rig.clock.time(),
            managed: false,
        })
        .unwrap(),
    )
    .unwrap();
    data.update_config(|config| config.update.auto = false)
        .unwrap();
    data
}

async fn refusal(rig: &Rig, connections: usize) {
    let mut status = rig.connector.as_ref().unwrap().link().status();
    tokio::time::timeout(PATIENCE, async {
        loop {
            if rig.hub.faults().connections() >= connections
                && matches!(
                    &*status.borrow(),
                    LinkStatus::Refused {
                        code: RejectCode::UpgradeRequired,
                        update_permit: Some(_),
                        ..
                    }
                )
            {
                return;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .expect("the authenticated minimum-version refusal never arrived");
}

#[tokio::test]
async fn automatic_refusal_permits_never_install_with_auto_off_after_reconnect_or_restart_but_an_explicit_permit_does()
 {
    let mut rig = Rig::with_roles(vec![Role::Node, Role::Agent]);
    rig.config.update.auto = false;
    rig.hub = Hub::with_versions(
        Arc::clone(&rig.clock),
        rig.pki.certificate(),
        ConnectorVersions {
            min: "1.0.0".into(),
            recommended: "2.0.0".into(),
        },
    );
    rig.controller = ScriptedController::attach(
        &rig.hub,
        Arc::clone(&rig.clock),
        rig.machine_id,
        rig.roles.clone(),
    );
    rig.hub.update_permit(
        rig.machine_id,
        UpdatePermit {
            recommended: "2.0.0".into(),
            automatic: true,
        },
    );
    let data = physical_data(&rig);
    let install = Install::default();
    for _ in 0..2 {
        let first = rig.hub.faults().connections() + 1;
        rig.start_link().await;
        refusal(&rig, first).await;
        {
            let connector = rig.connector.as_ref().unwrap();
            let runner = automatic::until_installed(connector, &data, Some(&install));
            tokio::pin!(runner);
            // Poll the actual supervisor with an already retained authenticated permit.
            tokio::select! {biased; _=&mut runner=>panic!("Auto off installed an automatic minimum-version grant"), ()=tokio::task::yield_now()=>{}}
            let next = rig.hub.faults().connections() + 1;
            rig.hub.faults().sever_now();
            tokio::select! {biased; _=&mut runner=>panic!("reconnecting bypassed Auto off"), ()=refusal(&rig,next)=>{}}
            assert_eq!(install.prepared.load(Ordering::SeqCst), 0);
            assert_eq!(install.installed.load(Ordering::SeqCst), 0);
            assert!(!connector.services().policy.policy().unwrap().update_auto);
        }
        rig.kill().await;
    }
    // The same real installation seam must work for an explicit authenticated request while Auto stays off.
    rig.hub.update_permit(
        rig.machine_id,
        UpdatePermit {
            recommended: "2.0.0".into(),
            automatic: false,
        },
    );
    let first = rig.hub.faults().connections() + 1;
    rig.start_link().await;
    refusal(&rig, first).await;
    let guard = tokio::time::timeout(
        PATIENCE,
        automatic::until_installed(rig.connector.as_ref().unwrap(), &data, Some(&install)),
    )
    .await
    .expect("an explicit minimum-version grant never installed");
    assert_eq!(install.prepared.load(Ordering::SeqCst), 1);
    assert_eq!(install.installed.load(Ordering::SeqCst), 1);
    assert!(
        !rig.connector
            .as_ref()
            .unwrap()
            .services()
            .policy
            .policy()
            .unwrap()
            .update_auto
    );
    rig.connector.take().unwrap().stop().await;
    drop(guard);
}
