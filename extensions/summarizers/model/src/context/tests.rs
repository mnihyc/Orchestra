use super::*;
use orch_summarizer_contract::{
    activity::ActivityKind, descriptor::SummaryField, summarizer::SummaryLimits,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn item(key: usize, text: String) -> InputItem {
    InputItem {
        key: key.to_string(),
        item: ActivityItem {
            kind: ActivityKind::AgentMessage,
            text,
            at: None,
        },
    }
}
fn history(count: usize) -> Vec<InputItem> {
    (0..count)
        .map(|key| item(key, format!("record {key} {}", "字".repeat(300))))
        .collect()
}
fn prompt(activity: Vec<ActivityItem>) -> Result<(String, String), Box<dyn std::error::Error>> {
    let config = crate::descriptor::Config::read(
        &json!({"provider_id":orch_types::new_id(),"model_name":"small"}),
    )?;
    let request = SummaryRequest {
        field: SummaryField::Summary,
        node: serde_json::from_value(
            json!({"id":orch_types::new_id(),"kind":"thread","name":"Work","redacted":false}),
        )?,
        previous: String::new(),
        activity,
        tag_choices: vec![],
        limits: SummaryLimits::for_field(SummaryField::Summary, 0),
        config: serde_json::to_value(&config)?,
    };
    Ok(prompts::render(&request, &config))
}

#[test]
fn a_short_history_goes_whole_and_counts_no_loss() -> TestResult {
    let input = vec![
        item(1, "First complete message.".into()),
        item(2, "第二条完整消息。".into()),
    ];
    let original: Vec<_> = input.iter().map(|input| input.item.clone()).collect();
    let filled = fill(input, Budget::new(32_000, 100)?, None, 2)?;
    assert_eq!(filled.activity, original);
    assert_eq!(
        filled.facts,
        SummaryInputFacts {
            included_items: 2,
            shortened_items: 0,
            omitted_items: 0
        }
    );
    Ok(())
}

#[test]
fn a_long_history_fills_the_budget_with_the_newest_items_at_a_common_level() -> TestResult {
    let filled = fill(history(100), Budget::new(1024, 0)?, None, 100)?;
    assert_eq!(filled.activity.len(), 16);
    assert_eq!(filled.plan.cuts[0].key, "84");
    assert_eq!(filled.plan.cuts[15].key, "99");
    assert_eq!(filled.activity.iter().map(item_tokens).sum::<u32>(), 1024);
    assert_eq!(
        filled.facts,
        SummaryInputFacts {
            included_items: 16,
            shortened_items: 16,
            omitted_items: 84
        }
    );
    assert!(filled.activity.iter().all(|item| item_tokens(item) == 64));
    Ok(())
}

#[test]
fn one_huge_item_cannot_crowd_out_recent_small_items() -> TestResult {
    let mut input = vec![item(0, "x".repeat(20_000))];
    input.extend((1..=20).map(|key| item(key, format!("brief {key:02}"))));
    let filled = fill(input, Budget::new(256, 0)?, None, 21)?;
    assert_eq!(filled.facts.included_items, 21);
    assert_eq!(filled.facts.shortened_items, 1);
    assert_eq!(filled.facts.omitted_items, 0);
    assert_eq!(filled.activity.iter().map(item_tokens).sum::<u32>(), 256);
    assert!(filled.activity[0].text.ends_with('…'));
    assert_eq!(filled.activity[20].text, "brief 20");
    Ok(())
}

#[test]
fn consecutive_short_refreshes_share_the_prompt_prefix_up_to_the_new_item() -> TestResult {
    let budget = Budget::new(512, 0)?;
    let before = fill(
        vec![item(0, "first".into()), item(1, "second".into())],
        budget,
        None,
        2,
    )?;
    let after = fill(
        vec![
            item(0, "first".into()),
            item(1, "second".into()),
            item(2, "third".into()),
        ],
        budget,
        Some(&before.plan),
        3,
    )?;
    let (old_system, old) = prompt(before.activity)?;
    let (new_system, new) = prompt(after.activity)?;
    assert_eq!(old_system, new_system);
    assert!(
        new.starts_with(
            old.strip_suffix("\n</activity>")
                .ok_or("missing activity frame")?
        )
    );
    Ok(())
}

#[test]
fn a_full_window_trims_an_old_chunk_and_keeps_surviving_cuts_for_later_appends() -> TestResult {
    let budget = Budget::new(512, 0)?;
    let first = fill(history(3), budget, None, 3)?;
    let mut next = history(3);
    next.push(item(3, "new small item".into()));
    let second = fill(next, budget, Some(&first.plan), 4)?;
    assert!(second.facts.omitted_items > 0);
    let retained: Vec<_> = second
        .plan
        .cuts
        .iter()
        .filter(|cut| cut.key != "3")
        .collect();
    for cut in retained {
        assert!(
            first.plan.cuts.contains(cut),
            "survivors keep their exact original cuts"
        );
    }
    let mut next = history(3);
    next.push(item(3, "new small item".into()));
    next.push(item(4, "next small item".into()));
    let third = fill(next, budget, Some(&second.plan), 5)?;
    assert_eq!(
        &third.activity[..second.activity.len()],
        second.activity.as_slice()
    );
    let (_, old) = prompt(second.activity)?;
    let (_, new) = prompt(third.activity)?;
    assert!(
        new.starts_with(
            old.strip_suffix("\n</activity>")
                .ok_or("missing activity frame")?
        )
    );
    Ok(())
}

#[test]
fn source_changes_and_budget_changes_replan_current_words_without_reusing_stored_text() -> TestResult
{
    let first = fill(history(3), Budget::new(512, 0)?, None, 3)?;
    let mut changed = history(3);
    changed[1].item.text = "The current redacted replacement.".into();
    let after = fill(changed, Budget::new(512, 0)?, Some(&first.plan), 3)?;
    assert!(
        after
            .activity
            .iter()
            .any(|item| item.text == "The current redacted replacement.")
    );
    assert_eq!(after.plan.cuts[0], first.plan.cuts[0]);
    assert_eq!(after.plan.cuts[2], first.plan.cuts[2]);
    let smaller = fill(history(3), Budget::new(128, 0)?, Some(&first.plan), 3)?;
    assert!(smaller.activity.iter().map(item_tokens).sum::<u32>() <= 128);
    assert_eq!(smaller.facts.included_items, 2);
    Ok(())
}

#[test]
fn escaping_and_multibyte_cuts_stay_within_the_rendered_budget() -> TestResult {
    for tokens in [64, 65, 100, 255, 512] {
        let input = vec![
            item(0, "<&字🙂>".repeat(300)),
            item(1, "short whole".into()),
        ];
        let filled = fill(input, Budget::new(tokens, 0)?, None, 2)?;
        assert!(filled.activity.iter().map(item_tokens).sum::<u32>() <= tokens);
        let (_, input) = prompt(filled.activity)?;
        assert!(!input.contains("<&字"));
    }
    Ok(())
}

#[test]
fn the_context_default_can_grow_only_within_the_declared_window_less_unchanged_output() -> TestResult
{
    assert_eq!(fit_window(32_000, 120, None)?, 32_000);
    assert_eq!(fit_window(32_000, 120, Some(16_000))?, 15_880);
    assert_eq!(fit_window(100_000, 120, Some(128_000))?, 100_000);
    assert_eq!(fit_window(200_000, 120, Some(128_000))?, 127_880);
    assert!(fit_window(32_000, 120, Some(120)).is_err());
    Ok(())
}
