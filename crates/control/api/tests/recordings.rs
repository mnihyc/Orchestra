//! The last output of a shell is put away in the store of content with the values of the environment's secrets taken out of it, read back by its hash, and not kept at all where its secrets cannot be opened to take them out.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use orch_api_contract::terminals::TERMINAL_RECORDING_MAX_BYTES;
use orch_api_core::blobs::Blobs;
use orch_api_core::recordings::KeptOutput;
use orch_blob_contract::store::{BlobStore, read_all, stream_of};
use orch_db::audit::{Actor, Origin};
use orch_db::secrets::MasterKey;
use orch_engine::shells::{MASKED, Recordings};
use orch_link_contract::commands::BlobRef;
use orch_settings::secrets::{Owner, Put, put};
use orch_types::Hash;
use std::sync::Arc;
use workspace::Site;

/// A place that keeps recordings, over a store in memory and the site's database.
struct Place {
    kept: KeptOutput,
    store: Arc<dyn BlobStore>,
}

impl Place {
    fn of(site: &Site, master: Option<MasterKey>) -> Self {
        let store = Blobs::in_memory().store();
        Self {
            kept: KeptOutput::new(Arc::clone(&store), site.world.app.app.db.clone(), master),
            store,
        }
    }

    /// Content as a machine uploaded it, and its reference.
    async fn uploaded(&self, content: &[u8]) -> BlobRef {
        let hash = Hash::of(content);
        let size = i64::try_from(content.len()).unwrap();
        self.store
            .put(
                &hash,
                Some(size),
                stream_of(vec![bytes::Bytes::copy_from_slice(content)]),
            )
            .await
            .unwrap();
        BlobRef { hash, size }
    }

    async fn text(&self, blob: &BlobRef) -> Vec<u8> {
        read_all(self.store.get(&blob.hash, None).await.unwrap().body)
            .await
            .unwrap()
            .to_vec()
    }
}

fn master() -> MasterKey {
    MasterKey::new([7; 32])
}

/// Gives the site's environment a secret of its own.
async fn a_secret(site: &Site, name: &str, value: &str) {
    let origin = Origin::default();
    let mut conn = site.world.conn().await;
    put(
        &mut conn,
        &master(),
        &Put {
            actor: Actor::User(site.ada.id()),
            owner: Owner::Environment(site.environment),
            name,
            value,
            origin: &origin,
        },
        site.world.now(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_recording_is_put_away_with_the_values_of_the_environments_secrets_taken_out() {
    let site = Site::new().await;
    a_secret(&site, "DEPLOY_TOKEN", "tok-4f9c1a77").await;
    let place = Place::of(&site, Some(master()));
    let uploaded = place
        .uploaded(b"$ deploy --token tok-4f9c1a77\nuploaded 3 files\n")
        .await;

    let kept = place
        .kept
        .keep(site.environment, uploaded, true)
        .await
        .unwrap();

    assert_ne!(
        kept.hash, uploaded.hash,
        "what is kept is a text of its own"
    );
    let text = String::from_utf8(place.text(&kept).await).unwrap();
    assert_eq!(
        text,
        format!("$ deploy --token {MASKED}\nuploaded 3 files\n"),
        "the value is gone and the rest is as it was"
    );
    assert_eq!(kept.size, i64::try_from(text.len()).unwrap());
    assert!(
        !String::from_utf8(place.text(&uploaded).await)
            .unwrap()
            .is_empty(),
        "the machine's upload is left where it is"
    );
}

#[tokio::test]
async fn a_recording_with_no_secret_in_it_is_kept_as_the_machine_uploaded_it() {
    let site = Site::new().await;
    a_secret(&site, "DEPLOY_TOKEN", "tok-4f9c1a77").await;
    let place = Place::of(&site, Some(master()));
    let uploaded = place.uploaded(b"$ ls\nREADME.md\n").await;

    let kept = place
        .kept
        .keep(site.environment, uploaded, true)
        .await
        .unwrap();

    assert_eq!(
        kept, uploaded,
        "nothing had to be replaced, and nothing is written"
    );
}

#[tokio::test]
async fn a_recording_of_an_environment_without_secrets_is_kept_as_it_was_uploaded() {
    let site = Site::new().await;
    let place = Place::of(&site, None);
    let uploaded = place.uploaded(b"$ whoami\nada\n").await;

    let kept = place
        .kept
        .keep(site.environment, uploaded, true)
        .await
        .unwrap();

    assert_eq!(
        kept, uploaded,
        "there is nothing to take out, and no key is needed for that"
    );
}

#[tokio::test]
async fn a_recording_is_kept_as_it_is_where_the_place_of_the_shell_does_not_mask() {
    let site = Site::new().await;
    a_secret(&site, "DEPLOY_TOKEN", "tok-4f9c1a77").await;
    let place = Place::of(&site, Some(master()));
    let uploaded = place.uploaded(b"$ echo tok-4f9c1a77\ntok-4f9c1a77\n").await;

    let kept = place
        .kept
        .keep(site.environment, uploaded, false)
        .await
        .unwrap();

    assert_eq!(kept, uploaded);
}

#[tokio::test]
async fn a_recording_whose_environment_has_secrets_that_cannot_be_opened_is_not_kept() {
    let site = Site::new().await;
    a_secret(&site, "DEPLOY_TOKEN", "tok-4f9c1a77").await;
    let place = Place::of(&site, None);
    let uploaded = place.uploaded(b"$ echo tok-4f9c1a77\ntok-4f9c1a77\n").await;

    let refused = place.kept.keep(site.environment, uploaded, true).await;

    assert!(
        refused.is_err(),
        "keeping it unmasked is what masking is for: {refused:?}"
    );
}

#[tokio::test]
async fn a_recording_longer_than_may_be_put_away_is_refused() {
    let site = Site::new().await;
    a_secret(&site, "DEPLOY_TOKEN", "tok-4f9c1a77").await;
    let place = Place::of(&site, Some(master()));
    let too_long = BlobRef {
        hash: Hash::of(b"long"),
        size: TERMINAL_RECORDING_MAX_BYTES + 1,
    };

    assert!(
        place
            .kept
            .keep(site.environment, too_long, true)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_recording_is_read_back_by_its_hash_and_a_hash_nobody_stored_reads_as_nothing() {
    let site = Site::new().await;
    let place = Place::of(&site, Some(master()));
    let uploaded = place.uploaded(b"$ date\nMon Oct 12\n").await;

    assert_eq!(
        place.kept.read(&uploaded.hash).await.unwrap(),
        Some(b"$ date\nMon Oct 12\n".to_vec())
    );
    assert_eq!(
        place.kept.read(&Hash::of(b"never stored")).await.unwrap(),
        None
    );
}
