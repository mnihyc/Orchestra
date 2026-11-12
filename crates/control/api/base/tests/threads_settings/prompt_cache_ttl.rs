//! The same inherited setting survives program support changes and says when it does not apply.
use super::*;
use orch_api_contract::settings::{NodeSettingList, NodeSettingSet, NodeSettingSetBody};

#[tokio::test]
async fn native_cache_lifetime_stays_inherited_and_declared_unsupported_programs_are_named_without_erasing_it()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let read = || {
        s.rig
            .world
            .app
            .call(NodeSettingList, &[&thread])
            .token(&s.ada.token)
    };
    let rows = read().send().await.ok();
    let row = rows
        .iter()
        .find(|row| row.key == "agents.prompt_cache_ttl")
        .unwrap();
    assert!(row.value.as_ref().is_none_or(serde_json::Value::is_null));
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.project.base.id),
            "agents.prompt_cache_ttl",
            json!("1h"),
        )
        .await;
    for (program, supported) in [
        ("Codex", false),
        ("DeepSeek Harness", false),
        ("Claude Code", true),
    ] {
        let mut descriptor = rig::descriptor();
        descriptor["harness"]["name"] = json!(program);
        descriptor["capabilities"]["prompt_cache_ttl"] = json!(supported);
        sqlx::query("UPDATE agents SET descriptor=$2,descriptor_overrides='{}' WHERE id=$1")
            .bind(s.on.agent)
            .bind(descriptor)
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        let rows = read().send().await.ok();
        let row = rows
            .iter()
            .find(|row| row.key == "agents.prompt_cache_ttl")
            .unwrap();
        assert_eq!(
            row.value,
            Some(json!("1h")),
            "support is a declared fact and never deletes the inherited choice"
        );
        assert_eq!(
            row.not_applicable_reason.as_deref(),
            (!supported).then_some("prompt_cache_ttl_unsupported")
        );
        assert_eq!(
            row.not_applicable_program.as_deref(),
            (!supported).then_some(program)
        );
    }
    let written = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &"agents.prompt_cache_ttl"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!("5m"),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .full_setting()
        .ok();
    assert_eq!(written.value, Some(json!("5m")));
    let inherited = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &"agents.prompt_cache_ttl"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(null),
            locked: None,
            expected_revision: written.own.as_ref().map(|row| row.revision),
        })
        .send()
        .await
        .full_setting()
        .ok();
    assert!(
        inherited
            .value
            .as_ref()
            .is_none_or(serde_json::Value::is_null),
        "an explicit unset selects the program default even below an inherited lifetime"
    );
}
