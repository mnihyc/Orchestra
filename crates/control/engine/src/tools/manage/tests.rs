use super::*;
use orch_types::new_id;

/// The seconds a word is good for in these tests: ten minutes.
const KEPT_SECS: i64 = 600;

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + Duration::seconds(seconds)
}

fn asked(command: Command, path: &str) -> Asked {
    Asked {
        command,
        paths: vec![path.to_owned()],
        to: None,
        name: None,
    }
}

fn waiting(made: DateTime<Utc>, asked: &Asked) -> Waiting {
    Waiting {
        made,
        asked: asked.clone(),
        stamp: None,
    }
}

#[test]
fn a_word_is_128_random_bits_written_in_32_characters_and_two_are_never_alike() {
    let words: HashSet<String> = (0..64).map(|_| new_word()).collect();
    assert_eq!(words.len(), 64);
    for word in &words {
        assert_eq!(word.len(), 32, "{word}");
        assert!(
            word.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{word}"
        );
    }
}

#[test]
fn a_word_is_spent_by_the_call_that_uses_it_and_is_good_for_one_thread_and_one_list() {
    let (thread, other) = (new_id(), new_id());
    let list = asked(Command::Delete, "docs/Plan.md");
    let mut plans = Plans::new(std::time::Duration::from_secs(
        u64::try_from(KEPT_SECS).unwrap(),
    ));
    plans.keep(thread, "abc", waiting(at(0), &list), at(0));
    // Another thread's call finds nothing, and does not spend it.
    assert!(matches!(
        plans.claim(other, "abc", &list, at(1)),
        Claim::Missing
    ));
    // Other arguments find it is not theirs, and do not spend it either.
    let other_list = asked(Command::Delete, "docs/Notes.md");
    assert!(matches!(
        plans.claim(thread, "abc", &other_list, at(1)),
        Claim::Other
    ));
    assert!(matches!(
        plans.claim(thread, "abc", &list, at(2)),
        Claim::Claimed(_)
    ));
    // Used: gone.
    assert!(matches!(
        plans.claim(thread, "abc", &list, at(3)),
        Claim::Missing
    ));
    assert!(matches!(
        plans.claim(thread, "made up", &list, at(3)),
        Claim::Missing
    ));
}

#[test]
fn a_word_is_good_for_ten_minutes_and_not_a_second_more() {
    let thread = new_id();
    let list = asked(Command::Restore, "docs/Plan.md");
    let mut plans = Plans::new(std::time::Duration::from_secs(
        u64::try_from(KEPT_SECS).unwrap(),
    ));
    plans.keep(thread, "abc", waiting(at(0), &list), at(0));
    assert!(matches!(
        plans.claim(thread, "abc", &list, at(KEPT_SECS - 1)),
        Claim::Claimed(_)
    ));
    plans.keep(thread, "def", waiting(at(0), &list), at(0));
    assert!(matches!(
        plans.claim(thread, "def", &list, at(KEPT_SECS)),
        Claim::Missing
    ));
}

#[test]
fn a_thread_that_asks_without_end_keeps_its_latest_plans_and_loses_the_oldest() {
    let thread = new_id();
    let mut plans = Plans::new(std::time::Duration::from_secs(
        u64::try_from(KEPT_SECS).unwrap(),
    ));
    for n in 0..(PLANS_PER_THREAD_MAX + 4) {
        let list = asked(Command::Delete, &format!("docs/{n}.md"));
        plans.keep(
            thread,
            &format!("w{n}"),
            waiting(at(i64::try_from(n).unwrap()), &list),
            at(i64::try_from(n).unwrap()),
        );
    }
    assert_eq!(plans.waiting_for(thread), PLANS_PER_THREAD_MAX);
    let first = asked(Command::Delete, "docs/0.md");
    assert!(matches!(
        plans.claim(thread, "w0", &first, at(30)),
        Claim::Missing
    ));
    let last = asked(
        Command::Delete,
        &format!("docs/{}.md", PLANS_PER_THREAD_MAX + 3),
    );
    assert!(matches!(
        plans.claim(
            thread,
            &format!("w{}", PLANS_PER_THREAD_MAX + 3),
            &last,
            at(30)
        ),
        Claim::Claimed(_)
    ));
}

#[test]
fn a_call_said_twice_is_answered_the_same_for_ten_minutes() {
    let thread = new_id();
    let mut plans = Plans::new(std::time::Duration::from_secs(
        u64::try_from(KEPT_SECS).unwrap(),
    ));
    assert!(
        plans
            .replay(
                thread,
                "call_1",
                &asked(Command::Delete, "docs/Plan.md"),
                at(0)
            )
            .is_none()
    );
    let list = asked(Command::Delete, "docs/Plan.md");
    let answer = Answer::Done(json!({ "done": false, "confirm": "abc" }));
    plans.remember(thread, "call_1", &list, at(0), &answer);
    assert_eq!(
        plans.replay(
            thread,
            "call_1",
            &asked(Command::Delete, "docs/Plan.md"),
            at(5)
        ),
        Some(answer)
    );
    assert!(
        plans.replay(new_id(), "call_1", &list, at(5)).is_none(),
        "another thread's call of that reference is another call"
    );
    assert!(
        plans
            .replay(
                thread,
                "call_1",
                &asked(Command::Delete, "docs/Plan.md"),
                at(KEPT_SECS)
            )
            .is_none()
    );
}

#[test]
fn the_arguments_of_a_command_are_checked_for_what_a_schema_cannot_say() {
    let read = |arguments: Json| asked_of(&arguments);
    let (delete, word) = read(json!({ "command": "delete", "paths": [" docs/Plan.md "] })).unwrap();
    assert_eq!(
        delete.paths,
        ["docs/Plan.md"],
        "a path is read without its spaces"
    );
    assert_eq!(word, None);
    let (moved, word) =
        read(json!({ "command": "move", "paths": ["docs/Plan.md"], "to": "../Other/docs" }))
            .unwrap();
    assert_eq!(moved.to.as_deref(), Some("../Other/docs"));
    assert!(word.is_none());
    for command in ["move", "restore"] {
        let arguments = if command == "move" {
            json!({ "command": command, "paths": ["docs/Plan.md"], "to": "../Other/docs", "confirm": "abc" })
        } else {
            json!({ "command": command, "paths": ["docs/Plan.md"], "confirm": "abc" })
        };
        assert_eq!(read(arguments).unwrap_err().code, Code::Invalid);
    }
    let no_place = read(json!({ "command": "move", "paths": ["docs/Plan.md"] })).unwrap_err();
    assert_eq!(no_place.code, Code::Invalid);
    assert!(
        no_place.message.contains("copy or move needs \"to\""),
        "{}",
        no_place.message
    );
    for command in ["delete", "restore"] {
        let wrong =
            read(json!({ "command": command, "paths": ["a.md"], "to": "docs" })).unwrap_err();
        assert_eq!(wrong.code, Code::Invalid);
        assert!(
            wrong.message.contains("\"to\" is only for copy and move"),
            "{}",
            wrong.message
        );
    }
    let empty = read(json!({ "command": "delete", "paths": ["a.md", "  "] })).unwrap_err();
    assert!(
        empty.message.contains("entry 2 is empty"),
        "{}",
        empty.message
    );
    for paths in [
        vec!["docs/research/", "docs/research/Plan.md"],
        vec!["docs/Plan.md", "docs/./Plan.md"],
    ] {
        let overlap = read(json!({ "command": "delete", "paths": paths })).unwrap_err();
        assert_eq!(overlap.code, Code::Invalid);
        assert!(overlap.message.contains("must not overlap"));
    }
    // A relative parent step is resolved against the actual tree, not guessed by a prefix.
    assert!(read(json!({ "command": "move", "paths": ["docs/research/", "docs/research/../Plan.md"], "to": "../Other/docs/" })).is_ok());
}
