//! Immutable stock image selections written into the release manifest.

use super::ReleaseError;
use super::manifest::Image;
use std::fs;
use std::path::Path;

// SHA-256 contains 256 bits, written as exactly 64 hexadecimal characters.
const SHA256_HEX_BYTES: usize = 64;

fn selected(reference: &str) -> Result<Image, ReleaseError> {
    let invalid = || {
        ReleaseError::Invalid(format!(
            "stock image {reference:?} needs a tag and a sha256 index digest"
        ))
    };
    let (tagged, digest) = reference.split_once('@').ok_or_else(invalid)?;
    let hex = digest.strip_prefix("sha256:").ok_or_else(invalid)?;
    if hex.len() != SHA256_HEX_BYTES
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid());
    }
    let (name, tag) = tagged
        .rsplit('/')
        .next()
        .and_then(|name| name.split_once(':'))
        .ok_or_else(invalid)?;
    if name.is_empty() || tag.is_empty() {
        return Err(invalid());
    }
    Ok(Image {
        name: name.to_owned(),
        reference: reference.to_owned(),
        digest: digest.to_owned(),
    })
}

/// Read the immutable image references the Compose services select.
pub(super) fn compose_images(text: &str) -> Result<Vec<Image>, ReleaseError> {
    let images = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("image:"))
        .map(|image| selected(image.trim().trim_matches(['"', '\''])))
        .collect::<Result<Vec<_>, _>>()?;
    if images.is_empty() {
        return Err(ReleaseError::Invalid(
            "the Compose file selects no stock images".to_owned(),
        ));
    }
    Ok(images)
}

/// Record the stock runtime images and the default environment build base.
pub(super) fn images(root: &Path) -> Result<Vec<Image>, ReleaseError> {
    let read = |relative: &str| {
        let path = root.join(relative);
        fs::read_to_string(&path).map_err(|source| ReleaseError::Io { path, source })
    };
    let compose = read("deploy/compose.yml")?;
    let mut images = compose_images(&compose)?;
    if compose.contains("build: ./postgres") && !root.join("deploy/postgres/Dockerfile").exists() {
        return Err(ReleaseError::Invalid(
            "the database build recipe is missing".to_owned(),
        ));
    }
    let recipe = read("images/orch-env-base/Dockerfile")?;
    let base = recipe
        .lines()
        .find_map(|line| line.trim().strip_prefix("ARG BASE_IMAGE="))
        .ok_or_else(|| {
            ReleaseError::Invalid("the environment recipe selects no default base image".to_owned())
        })?;
    let base = selected(base.trim().trim_matches(['"', '\'']))?;
    if !images.iter().any(|image| image.reference == base.reference) {
        images.push(base);
    }
    // The database image is made by Compose from a recipe of its own; the PostgreSQL image it starts from is
    // pinned there and recorded like the others.
    if root.join("deploy/postgres/Dockerfile").exists() {
        let recipe = read("deploy/postgres/Dockerfile")?;
        let base = recipe
            .lines()
            .find_map(|line| line.trim().strip_prefix("ARG POSTGRES_IMAGE="))
            .ok_or_else(|| {
                ReleaseError::Invalid("the database recipe selects no PostgreSQL image".to_owned())
            })?;
        let base = selected(base.trim().trim_matches(['"', '\'']))?;
        if !images.iter().any(|image| image.reference == base.reference) {
            images.push(base);
        }
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moving_stock_tag_prevents_packaging_a_release() {
        let fault = compose_images("services:\n  api:\n    image: debian:bookworm-slim\n")
            .expect_err("a moving tag must be refused");
        assert!(
            fault
                .to_string()
                .contains("needs a tag and a sha256 index digest")
        );
    }

    #[test]
    fn the_release_records_the_selected_stock_index_and_reuses_it_for_the_build_base() {
        let directory = tempfile::tempdir().expect("a directory");
        fs::create_dir_all(directory.path().join("deploy")).expect("a directory");
        fs::create_dir_all(directory.path().join("images/orch-env-base")).expect("a directory");
        let digest = format!("sha256:{}", "a".repeat(SHA256_HEX_BYTES));
        let reference = format!("debian:bookworm-slim@{digest}");
        fs::write(
            directory.path().join("deploy/compose.yml"),
            format!("services:\n  api:\n    image: {reference}\n"),
        )
        .expect("a Compose file");
        fs::write(
            directory.path().join("images/orch-env-base/Dockerfile"),
            format!("ARG BASE_IMAGE={reference}\nFROM ${{BASE_IMAGE}}\n"),
        )
        .expect("a recipe");
        let recorded = images(directory.path()).expect("immutable images");
        let json = serde_json::to_value(recorded).expect("manifest images");
        assert_eq!(json.as_array().map(Vec::len), Some(1));
        assert_eq!(json[0]["reference"], reference);
        assert_eq!(json[0]["digest"], digest);
        // The database recipe's PostgreSQL image is recorded beside them.
        fs::create_dir_all(directory.path().join("deploy/postgres")).expect("a directory");
        let database = format!("postgres:17-alpine@sha256:{}", "b".repeat(SHA256_HEX_BYTES));
        fs::write(
            directory.path().join("deploy/postgres/Dockerfile"),
            format!("ARG POSTGRES_IMAGE={database}\nFROM ${{POSTGRES_IMAGE}}\n"),
        )
        .expect("a recipe");
        let recorded = images(directory.path()).expect("immutable images");
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[1].reference, database);
        fs::write(
            directory.path().join("deploy/postgres/Dockerfile"),
            "FROM postgres:17-alpine\n",
        )
        .expect("a recipe");
        assert!(
            images(directory.path()).is_err(),
            "an unpinned database image is refused"
        );
    }
}
