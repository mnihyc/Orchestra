//! The browser's process and directory examples must parse under the server's actual grammar.

use orch_realtime_contract::feeds::Feed;
use orch_types::{Id, Json};

#[test]
fn browser_workspace_feeds_use_the_environment_and_preserve_the_directory()
-> Result<(), Box<dyn std::error::Error>> {
    let cases: Json = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../frontend/tests/fixtures/workspace-feeds.json"
    )))?;
    for row in cases.as_array().ok_or("feed examples are not an array")? {
        let environment: Id = row["environment"]
            .as_str()
            .ok_or("missing environment")?
            .parse()?;
        let path = row["path"].as_str().ok_or("missing path")?;
        let directory = row["directory_feed"]
            .as_str()
            .ok_or("missing directory feed")?;
        let processes = row["process_feed"].as_str().ok_or("missing process feed")?;
        assert_eq!(
            directory.parse::<Feed>()?,
            Feed::EnvDir {
                environment_id: environment,
                path: path.to_owned()
            }
        );
        assert_eq!(processes.parse::<Feed>()?, Feed::EnvProcs(environment));
        assert_eq!(directory.parse::<Feed>()?.to_string(), directory);
    }
    Ok(())
}
