use super::*;
use crate::schema::{Fault, MAX_FAULTS};
use serde_json::json;
use std::sync::{Barrier, atomic::Ordering};

fn compilations(cache: &ValidatorCache) -> usize {
    cache.compilations.load(Ordering::Relaxed)
}

fn retained(cache: &ValidatorCache) -> (usize, usize) {
    let state = cache
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (state.entries.len(), state.schema_bytes)
}

#[test]
fn repeated_checks_and_different_instances_compile_once_without_caching_acceptance() {
    let cache = ValidatorCache::default();
    let schema = json!({"type":"object","required":["mode"],"additionalProperties":false,
        "properties":{"mode":{"enum":["allowed"]}}});
    for _ in 0..3 {
        let same = schema.clone();
        assert!(cache.check(&same).is_ok());
        assert!(cache.validate(&schema, &json!({"mode":"allowed"})).is_ok());
        assert!(cache.validate(&schema, &json!({"mode":"refused"})).is_err());
        assert!(cache.validate(&schema, &json!({})).is_err());
        assert!(
            cache
                .validate(&schema, &json!({"mode":"allowed","extra":true}))
                .is_err()
        );
    }
    assert_eq!(compilations(&cache), 1);
    assert_eq!(retained(&cache).0, 1);
}

#[test]
fn exact_schema_edits_miss_while_unchanged_content_at_another_address_reuses() {
    let cache = ValidatorCache::default();
    let mut schema = json!({"type":"integer","minimum":2});
    assert!(cache.validate(&schema, &json!(2)).is_ok());
    schema["minimum"] = json!(3);
    assert!(cache.validate(&schema, &json!(2)).is_err());
    let same = schema.clone();
    assert!(cache.validate(&same, &json!(3)).is_ok());
    assert_eq!(compilations(&cache), 2);
    schema["description"] = json!("A revised declaration with the same numeric bound.");
    assert!(cache.validate(&schema, &json!(3)).is_ok());
    assert_eq!(compilations(&cache), 3, "the full exact content is the key");
}

#[test]
fn local_choices_share_their_pointer_validator_and_keep_the_complete_reference_root() {
    let cache = ValidatorCache::default();
    let mut schema = json!({"$defs":{"choice":{"enum":["on"]}},"type":"object",
        "properties":{"a~/b":{"$ref":"#/$defs/choice"},"other":{"enum":["off"]}}});
    let pointer = "/properties/a~0~1b";
    for _ in 0..3 {
        assert!(cache.allows_at(&schema, pointer, &json!("on")));
        assert!(!cache.allows_at(&schema, pointer, &json!("off")));
    }
    assert_eq!(compilations(&cache), 1);
    assert!(cache.allows_at(&schema, "/properties/other", &json!("off")));
    assert!(!cache.allows_at(&schema, "/properties/other", &json!("on")));
    assert!(
        cache
            .validate(&schema, &json!({"a~/b":"on","other":"off"}))
            .is_ok()
    );
    assert_eq!(
        compilations(&cache),
        3,
        "whole and distinct pointers stay separate"
    );
    // The selected property's text is unchanged; only its referenced definition changes.
    schema["$defs"]["choice"]["enum"] = json!(["off"]);
    assert!(!cache.allows_at(&schema, pointer, &json!("on")));
    assert!(cache.allows_at(&schema, pointer, &json!("off")));
    assert_eq!(compilations(&cache), 4);
}

#[test]
fn concurrent_first_users_share_one_compilation_and_each_checks_its_own_value() {
    let cache = ValidatorCache::default();
    let schema = json!({"type":"integer","minimum":4});
    let barrier = Barrier::new(8);
    std::thread::scope(|scope| {
        for value in 0..8 {
            let cache = &cache;
            let schema = &schema;
            let barrier = &barrier;
            let _worker = scope.spawn(move || {
                barrier.wait();
                assert_eq!(cache.validate(schema, &json!(value)).is_ok(), value >= 4);
            });
        }
    });
    assert_eq!(compilations(&cache), 1);
    assert_eq!(retained(&cache).0, 1);
}

#[test]
fn least_recently_used_entries_are_bounded_and_eviction_changes_only_reuse() {
    let cache = ValidatorCache::new(CacheLimits {
        entries: 2,
        ..CacheLimits::default()
    });
    let a = json!({"const":"a"});
    let b = json!({"const":"b"});
    let c = json!({"const":"c"});
    assert!(cache.validate(&a, &json!("a")).is_ok());
    assert!(cache.validate(&b, &json!("b")).is_ok());
    assert!(cache.validate(&a, &json!("b")).is_err());
    assert!(cache.validate(&c, &json!("c")).is_ok());
    assert_eq!(retained(&cache).0, 2);
    assert_eq!(compilations(&cache), 3);
    assert!(cache.validate(&a, &json!("a")).is_ok());
    assert_eq!(compilations(&cache), 3, "the recent entry remains compiled");
    assert!(cache.validate(&b, &json!("a")).is_err());
    assert_eq!(
        compilations(&cache),
        4,
        "the evicted grammar is compiled afresh"
    );
    assert_eq!(retained(&cache).0, 2);
}

#[test]
fn source_byte_and_zero_entry_limits_bypass_reuse_without_refusing_schemas() {
    let schema = json!({"type":"string","minLength":1});
    let bytes = schema.to_string().len();
    for limits in [
        CacheLimits {
            entries: 8,
            schema_bytes: bytes - 1,
        },
        CacheLimits {
            entries: 0,
            schema_bytes: bytes,
        },
    ] {
        let cache = ValidatorCache::new(limits);
        for _ in 0..2 {
            assert!(cache.validate(&schema, &json!("allowed")).is_ok());
            assert!(cache.validate(&schema, &json!("")).is_err());
        }
        assert_eq!(compilations(&cache), 4);
        assert_eq!(retained(&cache), (0, 0));
    }

    let a = json!({"const":"a"});
    let b = json!({"const":"b"});
    let source_bytes = a.to_string().len();
    let cache = ValidatorCache::new(CacheLimits {
        entries: 8,
        schema_bytes: source_bytes,
    });
    assert!(cache.check(&a).is_ok());
    assert!(cache.check(&b).is_ok());
    assert_eq!(retained(&cache), (1, source_bytes));
    assert!(cache.check(&a).is_ok());
    assert_eq!(compilations(&cache), 3);
    assert_eq!(retained(&cache), (1, source_bytes));

    let pointer = "/properties/value";
    let projected = json!({"properties":{"value":{"const":"on"}}});
    let cache = ValidatorCache::new(CacheLimits {
        entries: 8,
        schema_bytes: projected.to_string().len() + pointer.len() - 1,
    });
    assert!(cache.allows_at(&projected, pointer, &json!("on")));
    assert!(!cache.allows_at(&projected, pointer, &json!("off")));
    assert_eq!(retained(&cache), (0, 0), "pointer bytes count too");
}

#[test]
fn cached_and_uncached_faults_keep_formats_linear_patterns_local_only_and_redaction() {
    let cached = ValidatorCache::default();
    let uncached = ValidatorCache::new(CacheLimits {
        entries: 0,
        ..CacheLimits::default()
    });
    let schema = json!({"type":"object","required":["a~/b"],"additionalProperties":false,
        "properties":{"a~/b":{"type":"string","format":"email"},"code":{"type":"string","pattern":"^[a-z]+$"}}});
    for value in [
        json!({"a~/b":"person@example.invalid","code":"letters"}),
        json!({"a~/b":"private-instance-without-an-address","code":"123"}),
        json!({}),
        json!({"a~/b":"person@example.invalid","unexpected":"private-extra-value"}),
    ] {
        assert_eq!(
            cached.validate(&schema, &value),
            uncached.validate(&schema, &value)
        );
    }
    let Err(Error::Value { faults }) = cached.validate(&schema, &json!({})) else {
        panic!("a required member is still required");
    };
    assert_eq!(
        faults,
        vec![Fault {
            path: "/a~0~1b".into(),
            schema_path: "/required".into(),
            message: "a required member is missing".into(),
        }]
    );
    let invalid = json!({"a~/b":"private-instance-without-an-address","code":"123"});
    let result = cached.validate(&schema, &invalid);
    let Err(Error::Value { faults }) = &result else {
        panic!("format and pattern validation remain enabled");
    };
    assert!(
        faults
            .iter()
            .any(|fault| fault.schema_path.ends_with("/format"))
    );
    assert!(
        faults
            .iter()
            .any(|fault| fault.schema_path.ends_with("/pattern"))
    );
    assert!(!format!("{result:?}").contains("private-instance"));
    assert!(!cached.allows_at(&schema, "/properties/a~0~1b", &json!("private-instance")));
    assert!(cached.allows_at(
        &schema,
        "/properties/a~0~1b",
        &json!("person@example.invalid")
    ));

    for schema in [
        json!({"type":"not-a-type"}),
        json!({"type":"string","pattern":"^(a+)\\1$"}),
        json!({"$ref":"https://example.invalid/private-schema.json"}),
        json!({"$ref":"file:///private-schema.json"}),
    ] {
        assert!(matches!(cached.check(&schema), Err(Error::Schema { .. })));
        assert_eq!(cached.check(&schema), uncached.check(&schema));
        assert!(!cached.allows_at(&schema, "", &json!("aa")));
    }
}

#[test]
fn bad_declarations_are_cached_with_the_same_limits_and_fault_lists_stay_bounded() {
    let cache = ValidatorCache::new(CacheLimits {
        entries: 1,
        ..CacheLimits::default()
    });
    let bad = json!({"type":"not-a-type"});
    for _ in 0..3 {
        assert!(matches!(cache.check(&bad), Err(Error::Schema { .. })));
    }
    assert_eq!(compilations(&cache), 1);
    let schema = json!({"type":"object","required":(0..=MAX_FAULTS).map(|n| format!("field-{n}")).collect::<Vec<_>>()});
    let Err(Error::Value { faults }) = cache.validate(&schema, &json!({})) else {
        panic!("all missing fields still refuse the configuration");
    };
    assert_eq!(faults.len(), MAX_FAULTS);
    assert!(matches!(cache.check(&bad), Err(Error::Schema { .. })));
    assert_eq!(
        compilations(&cache),
        3,
        "failed compilation was evicted too"
    );
    assert_eq!(retained(&cache).0, 1);
}
