//! Certificate preparation keeps the old browser origin usable until publication succeeds.

use super::*;
use orch_api_contract::public_addresses::PublicAddresses;

#[tokio::test]
async fn a_new_frontend_generation_is_complete_and_covers_old_primary_and_all_candidate_origins() {
    let fixture = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::default();
    config.storage.root = root.path().to_string_lossy().into_owned();
    let old = PublicAddresses {
        public_url: "https://old.example.test".into(),
        alternate_urls: Vec::new(),
    };
    let candidate = PublicAddresses {
        public_url: "https://new.example.test".into(),
        alternate_urls: vec![
            "https://inside.example.test:8443".into(),
            "https://192.0.2.15".into(),
        ],
    };
    let generation = fixture
        .service
        .prepare_frontend_certificate(&config, &old, &candidate)
        .unwrap();
    let directory = root.path().join("frontend-tls");
    assert!(
        !directory.join("current").exists(),
        "preparing never changes the frontend's live pointer"
    );
    let chain = Certificate::all_from_pem(
        &std::fs::read_to_string(directory.join(&generation.name).join("chain.pem")).unwrap(),
    )
    .unwrap();
    let key = orch_pki::certificates::LeafKey::from_pem(
        &std::fs::read_to_string(directory.join(&generation.name).join("key.pem")).unwrap(),
    )
    .unwrap();
    assert!(chain[0].holds(&key));
    for name in [
        "old.example.test",
        "new.example.test",
        "inside.example.test",
    ] {
        assert!(chain[0].dns_names().contains(&name.to_owned()));
    }
    assert_eq!(chain[1], *fixture.authority.certificate());
    fixture
        .service
        .activate_frontend_certificate(&config, &generation)
        .unwrap();
    assert_eq!(
        std::fs::read_link(directory.join("current")).unwrap(),
        std::path::Path::new(&generation.name)
    );
}

#[tokio::test]
async fn issued_browser_flows_keep_their_old_certificate_name_after_later_publications_and_renewal()
{
    let fixture = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::default();
    config.storage.root = root.path().to_string_lossy().into_owned();
    let addresses = |name: &str| PublicAddresses {
        public_url: format!("https://{name}.example.test"),
        alternate_urls: Vec::new(),
    };
    let old = addresses("old");
    let next = addresses("next");
    let last = addresses("last");
    let first = fixture
        .service
        .prepare_frontend_certificate(&config, &old, &next)
        .unwrap();
    fixture
        .service
        .activate_frontend_certificate(&config, &first)
        .unwrap();
    let later = fixture
        .service
        .prepare_frontend_certificate(&config, &next, &last)
        .unwrap();
    fixture
        .service
        .activate_frontend_certificate(&config, &later)
        .unwrap();
    let directory = root.path().join("frontend-tls/current");
    let certificate = || {
        Certificate::all_from_pem(&std::fs::read_to_string(directory.join("chain.pem")).unwrap())
            .unwrap()
            .remove(0)
    };
    assert!(
        certificate()
            .dns_names()
            .contains(&"old.example.test".to_owned())
    );
    let before = orch_db::queries::public_addresses::read(
        &mut fixture.db.acquire().await.unwrap(),
        &last,
        clock::start(),
    )
    .await
    .unwrap();
    assert!(before.from_file_seed);
    fixture.clock.advance(chrono::Duration::days(250));
    fixture
        .service
        .keep_frontend_certificate(&config)
        .await
        .unwrap();
    for name in ["old.example.test", "next.example.test", "last.example.test"] {
        assert!(certificate().dns_names().contains(&name.to_owned()));
    }
}
