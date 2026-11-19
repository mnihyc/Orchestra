//! The combination matrix of same names: a second org made with the short name of a first one,
//! written the same, in capitals, or otherwise, or with a person's username, beside it (the same parent), in another
//! root org, inside the first, or as a root org, with the same display name or another.
//!
//! Expected: the same name is allowed in different orgs and parents and refused only where an
//! address would become ambiguous, among the siblings of one parent, case-folded; the root orgs share their namespace
//! with usernames (an address names a person or a root org); a refusal names the sibling that holds the name where the
//! caller may see it and offers another short name; a display name never needs to be unique. A short name written in
//! capitals is either refused as a short name (when short names are lower case) or compared case-folded: two siblings
//! whose short names differ only in case never both exist.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "../../engine/tests/matrix_kit/mod.rs"]
mod matrix_kit;
mod places;
mod support;

use matrix_kit::{Case, Matrix, Report, dim, failure};
use orch_api_contract::nodes::{OrgChildCreate, OrgChildCreateBody, OrgCreate, OrgCreateBody};
use orch_types::Id;
use places::{Who, invite_to_make_org, make_department, make_org, person};
use support::World;

const SOURCE: &str = "Case-folded sibling address uniqueness, readable conflict names and alternatives; display names may repeat";

struct Ground {
    world: World,
    ada: Who,
    acme: Id,
    beta: Id,
}

/// What a second making came to: its status, and the problem's words when refused.
async fn make_second(
    ground: &Ground,
    case: &Case,
    first: Id,
    slug: &str,
    name: &str,
) -> (u16, String) {
    let world = &ground.world;
    let parent = match case.get("where") {
        "same_parent" => Some(ground.acme),
        "other_root" => Some(ground.beta),
        "inside_first" => Some(first),
        _ => None,
    };
    match parent {
        Some(parent) => {
            let reply = world
                .app
                .call(OrgChildCreate, &[&parent])
                .token(&ground.ada.token)
                .body(&OrgChildCreateBody {
                    name: name.to_owned(),
                    slug: slug.to_owned(),
                    description: None,
                    general: None,
                    access: None,
                })
                .send()
                .await;
            (
                reply.status.as_u16(),
                reply
                    .body
                    .err()
                    .map(|problem| serde_json::to_string(&problem).unwrap())
                    .unwrap_or_default(),
            )
        }
        None => {
            let maker = person(world, &format!("mk{}", case.tag())).await;
            invite_to_make_org(world, &maker).await;
            let reply = world
                .app
                .call(OrgCreate, &[])
                .token(&maker.token)
                .body(&OrgCreateBody {
                    name: name.to_owned(),
                    slug: slug.to_owned(),
                })
                .send()
                .await;
            (
                reply.status.as_u16(),
                reply
                    .body
                    .err()
                    .map(|problem| serde_json::to_string(&problem).unwrap())
                    .unwrap_or_default(),
            )
        }
    }
}

async fn run(ground: &Ground, case: &Case) -> Result<(), matrix_kit::Failure> {
    let world = &ground.world;
    let base = format!("dep{}", case.tag());
    let first_name = format!("Dep {}", case.tag());
    // The first one: a department of Acme, or a root org of somebody's for the case of root orgs.
    let first = if case.is("where", "root") {
        let founder = person(world, &format!("fd{}", case.tag())).await;
        make_org(world, &founder, &first_name, &base).await.base.id
    } else {
        make_department(world, &ground.ada, ground.acme, &first_name, &base)
            .await
            .base
            .id
    };
    let username = format!("us{}", case.tag());
    let slug = match case.get("slug") {
        "exact" => base.clone(),
        "capitals" => base.to_uppercase(),
        "other" => format!("{base}b"),
        _ => {
            person(world, &username).await;
            username.clone()
        }
    };
    let name = if case.is("name", "same") {
        first_name.clone()
    } else {
        format!("Other {}", case.tag())
    };
    let (status, problem) = make_second(ground, case, first, &slug, &name).await;
    let namespace = matches!(case.get("where"), "same_parent" | "root");
    let created = (200..300).contains(&status);
    match case.get("slug") {
        "exact" if namespace => {
            if status != 409 {
                return Err(failure(
                    "refused as taken (409)",
                    SOURCE,
                    format!("{status} {problem}"),
                ));
            }
            if !problem.contains("suggestion") {
                return Err(failure("another short name offered", SOURCE, problem));
            }
            if case.is("where", "same_parent") && !problem.contains(&first_name) {
                return Err(failure(
                    format!("the sibling {first_name} named"),
                    SOURCE,
                    problem,
                ));
            }
            Ok(())
        }
        "capitals" if namespace => {
            if created {
                Err(failure(
                    "refused: the short name differs from a sibling's only in case",
                    SOURCE,
                    format!("{status} (made)"),
                ))
            } else {
                Ok(())
            }
        }
        "capitals" => {
            // Another parent: allowed, unless short names are lower case everywhere (then refused as a short name).
            if created || status == 400 || status == 422 {
                Ok(())
            } else {
                Err(failure(
                    "made, or refused only for its capitals",
                    SOURCE,
                    format!("{status} {problem}"),
                ))
            }
        }
        "username" if case.is("where", "root") => {
            if status == 409 {
                Ok(())
            } else {
                Err(failure(
                    "refused: a root org's address is a username's (409)",
                    SOURCE,
                    format!("{status} {problem}"),
                ))
            }
        }
        _ => {
            if created {
                Ok(())
            } else {
                Err(failure(
                    "made (the same name elsewhere, or another short name)",
                    SOURCE,
                    format!("{status} {problem}"),
                ))
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_name_is_refused_only_where_an_address_would_name_two_orgs() {
    let matrix = Matrix::new(
        "names",
        &[
            dim(
                "where",
                &["same_parent", "other_root", "inside_first", "root"],
            ),
            dim("slug", &["exact", "capitals", "other", "username"]),
            dim("name", &["same", "other"]),
        ],
        |_| true,
    );
    let mut cases = matrix.cases(17, &[&["where", "slug", "name"]], 20_261_015, 0);
    cases.retain(|case| case.kind == "full");
    let world = World::new().await;
    // A system admin makes both root orgs (each making uses up an invitation of an ordinary person).
    let ada = places::admin(&world, "ada").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await.base.id;
    let beta = make_org(&world, &ada, "Beta", "beta").await.base.id;
    let ground = Ground {
        world,
        ada,
        acme,
        beta,
    };
    let mut report = Report::default();
    for case in &cases {
        let outcome = run(&ground, case).await;
        report.record(case, outcome);
    }
    report.finish("names");
}
