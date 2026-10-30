//! How a release's version is spelled, so that one release has one name wherever it is compared.
//!
//! A Python index writes a pre-release `0.2.1rc2` (PEP 440) where the program prints, and a profile pins, `0.2.1-rc.2`.
//! Orch keeps a build under the second, which is also how semver and the vendors' own tags and registries write
//! it, and reads either spelling when two versions are compared.

/// The version a release is called everywhere in Orch: a Python index (PEP 440) names a pre-release `0.2.1rc2` where the program prints
/// and a profile pins `0.2.1-rc.2`, so the index's name is spelled the way semver spells it (`a` is `alpha`, `b` is
/// `beta`, `rc` and `c` are `rc`, `.devN` is `-dev.N`, `.postN` is build metadata `+post.N`). A version that is
/// already so spelled, or that has no pre-release part, is returned as it is.
pub fn semver_of(version: &str) -> String {
    let lower = version.trim().to_ascii_lowercase();
    if lower.contains('-') || lower.contains('+') {
        return version.trim().to_owned();
    }
    let release_end = lower
        .find(|each: char| !(each.is_ascii_digit() || each == '.'))
        .unwrap_or(lower.len());
    let (release, rest) = lower.split_at(release_end);
    let release = release.trim_end_matches('.');
    if rest.is_empty() || release.is_empty() {
        return version.trim().to_owned();
    }
    let mut pre = None;
    let mut dev = None;
    let mut post = None;
    let mut rest = rest;
    // Segments follow in PEP 440's order: pre-release, post-release, development release, each after an optional
    // separator, and each with an optional number (absent means zero).
    while !rest.is_empty() {
        let tail = rest.trim_start_matches(['.', '-', '_']);
        let word_end = tail
            .find(|each: char| !each.is_ascii_alphabetic())
            .unwrap_or(tail.len());
        let (word, after) = tail.split_at(word_end);
        let digits_end = after
            .find(|each: char| !each.is_ascii_digit())
            .unwrap_or(after.len());
        let (digits, next) = after.split_at(digits_end);
        let number = if digits.is_empty() { "0" } else { digits };
        match word {
            "a" | "alpha" if pre.is_none() => pre = Some(format!("alpha.{number}")),
            "b" | "beta" if pre.is_none() => pre = Some(format!("beta.{number}")),
            "c" | "rc" | "pre" | "preview" if pre.is_none() => pre = Some(format!("rc.{number}")),
            "post" | "rev" | "r" if post.is_none() => post = Some(format!("post.{number}")),
            "dev" if dev.is_none() => dev = Some(format!("dev.{number}")),
            // Something this does not know: the name is left as the index wrote it.
            _ => return version.trim().to_owned(),
        }
        rest = next;
    }
    let mut spelled = release.to_owned();
    let marks: Vec<String> = pre.into_iter().chain(dev).collect();
    if !marks.is_empty() {
        spelled.push('-');
        spelled.push_str(&marks.join("."));
    }
    if let Some(post) = post {
        spelled.push('+');
        spelled.push_str(&post);
    }
    spelled
}

/// The name the index knows a release by, from the version as Orch spells it: the reverse of [`semver_of`], for the
/// address of one release. A version with no pre-release or post-release part is the same in both.
pub fn pep440_of(version: &str) -> String {
    let version = version.trim();
    let (main, metadata) = version
        .split_once('+')
        .map_or((version, None), |(main, metadata)| (main, Some(metadata)));
    let (release, marks) = main.split_once('-').unwrap_or((main, ""));
    let mut spelled = release.to_owned();
    let mut words = marks.split('.').filter(|each| !each.is_empty()).peekable();
    while let Some(word) = words.next() {
        let short = match word {
            "alpha" => "a",
            "beta" => "b",
            "rc" => "rc",
            "dev" => ".dev",
            // Something this does not know: the version is left as it was given.
            _ => return version.to_owned(),
        };
        spelled.push_str(short);
        if let Some(number) = words.next_if(|next| next.chars().all(|each| each.is_ascii_digit())) {
            spelled.push_str(number);
        }
    }
    if let Some(number) = metadata.and_then(|metadata| metadata.strip_prefix("post.")) {
        spelled.push_str(".post");
        spelled.push_str(number);
    }
    spelled
}

/// Whether two names are one release: the same text, or the same once spelled alike.
pub fn same_release(one: &str, other: &str) -> bool {
    one == other || semver_of(one) == semver_of(other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pre_release_has_one_name_in_orch_whichever_way_the_index_or_a_pin_spells_it() {
        assert_eq!(semver_of("0.2.1rc2"), "0.2.1-rc.2");
        assert_eq!(semver_of("0.2.1-rc.2"), "0.2.1-rc.2", "already so spelled");
        assert_eq!(semver_of("1.0.0a3"), "1.0.0-alpha.3");
        assert_eq!(semver_of("1.0.0b2"), "1.0.0-beta.2");
        assert_eq!(semver_of("1.0.0.dev4"), "1.0.0-dev.4");
        assert_eq!(semver_of("1.0.0rc1.dev2"), "1.0.0-rc.1.dev.2");
        assert_eq!(semver_of("1.0.0.post1"), "1.0.0+post.1");
        assert_eq!(semver_of("1.0.0RC1"), "1.0.0-rc.1", "case does not matter");
        assert_eq!(semver_of("2.1.309"), "2.1.309", "a release has one name");
        assert_eq!(
            semver_of("1.0.0weird"),
            "1.0.0weird",
            "what is not known is left"
        );
        assert_eq!(pep440_of("0.2.1-rc.2"), "0.2.1rc2");
        assert_eq!(
            pep440_of("0.2.1rc2"),
            "0.2.1rc2",
            "the index's own name stays"
        );
        assert_eq!(pep440_of("1.0.0-alpha.3"), "1.0.0a3");
        assert_eq!(pep440_of("1.0.0-dev.4"), "1.0.0.dev4");
        assert_eq!(pep440_of("1.0.0+post.1"), "1.0.0.post1");
        assert_eq!(pep440_of("2.1.309"), "2.1.309");
        for name in [
            "0.2.1rc2",
            "1.0.0a3",
            "1.0.0b2",
            "1.0.0.dev4",
            "1.0.0",
            "1.0.0.post1",
        ] {
            assert_eq!(
                pep440_of(&semver_of(name)),
                name,
                "{name} goes there and back"
            );
        }
        assert!(same_release("0.2.1rc2", "0.2.1-rc.2"));
        assert!(!same_release("0.2.1rc2", "0.2.1-rc.3"));
        assert!(!same_release("0.2.1rc2", "0.2.1"));
    }
}
