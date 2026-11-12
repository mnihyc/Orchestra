//! Public bundled help, using the browser's article IDs and localized heading anchors.

use super::*;

use regex::Regex;

use std::sync::LazyLock;
// These retain the existing result/excerpt bounds; results alternate languages when both match.
const HITS: usize = 5;

const EXCERPT_CHARS: usize = 4_000;

fn slug(text: &str) -> ApiResult<String> {
    static PUNCTUATION: LazyLock<Result<Regex, regex::Error>> =
        LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}\s-]"));
    static SPACE: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| Regex::new(r"\s+"));
    let punctuation = PUNCTUATION.as_ref().map_err(ApiError::internal)?;
    let space = SPACE.as_ref().map_err(ApiError::internal)?;
    let lower = text.to_lowercase();
    let words = punctuation.replace_all(&lower, "");
    Ok(space.replace_all(words.trim(), "-").into_owned())
}

fn front_matter(source: &str) -> (BTreeMap<&str, &str>, &str) {
    if !source.starts_with("---\n") {
        return (BTreeMap::new(), source);
    }
    let Some(end) = source[4..].find("\n---").map(|end| end + 4) else {
        return (BTreeMap::new(), source);
    };
    let meta = source[4..end]
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            (!key.trim().is_empty()).then(|| (key.trim(), value.trim()))
        })
        .collect();
    (
        meta,
        source[end + 4..]
            .strip_prefix('\n')
            .unwrap_or(&source[end + 4..]),
    )
}

struct Heading<'a> {
    level: usize,
    text: &'a str,
    start: usize,
}

fn headings(body: &str) -> Vec<Heading<'_>> {
    let mut result = Vec::new();
    let mut fenced = false;
    let mut start = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        } else if !fenced {
            let level = line.bytes().take_while(|byte| *byte == b'#').count();
            if (1..=4).contains(&level)
                && line[level..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
            {
                let text = line[level..].trim().trim_end_matches('#').trim_end();
                if !text.is_empty() {
                    result.push(Heading { level, text, start });
                }
            }
        }
        start += line.len();
    }
    result
}

fn contains(text: &str, words: &[String]) -> bool {
    let text = text.to_lowercase();
    words.iter().all(|word| text.contains(word))
}

fn section<'a>(body: &'a str, headings: &[Heading<'_>], index: usize) -> &'a str {
    let heading = &headings[index];
    let end = headings[index + 1..]
        .iter()
        .find(|next| next.level <= heading.level)
        .map_or(body.len(), |next| next.start);
    &body[heading.start..end]
}

fn hit(name: &str, source: &str, words: &[String]) -> ApiResult<Option<Json>> {
    let normalized = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let (meta, body) = front_matter(&normalized);
    let fallback = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .trim_end_matches(".md");
    let article = meta.get("id").copied().unwrap_or(fallback);
    let headings = headings(body);
    let title = meta
        .get("title")
        .copied()
        .or_else(|| {
            headings
                .iter()
                .find(|heading| heading.level == 1)
                .map(|heading| heading.text)
        })
        .unwrap_or(article);
    let searchable = format!(
        "{title}\n{}\n{body}",
        meta.get("keywords").copied().unwrap_or_default()
    );
    if !contains(&searchable, words) {
        return Ok(None);
    }
    let candidates = || {
        headings
            .iter()
            .enumerate()
            .filter(|(_, heading)| heading.level > 1)
    };
    let selected = candidates()
        .find(|(_, heading)| contains(heading.text, words))
        .or_else(|| {
            candidates().find(|(index, _)| contains(section(body, &headings, *index), words))
        });
    let (heading, excerpt, url) = if let Some((index, heading)) = selected {
        let slug = slug(heading.text)?;
        let key = format!("heading_id.{slug}");
        let anchor = meta.get(key.as_str()).copied().unwrap_or(&slug);
        (
            Some(heading.text),
            section(body, &headings, index),
            format!("/help/{article}#{anchor}"),
        )
    } else {
        (None, body, format!("/help/{article}"))
    };
    Ok(Some(
        json!({"text":excerpt.chars().take(EXCERPT_CHARS).collect::<String>(),
        "title":heading.unwrap_or(title),"article_title":title,"heading":heading,
        "language":if name.starts_with("zh-CN/") {"zh-CN"} else {"en"},
        "url":url,"reference":url}),
    ))
}

pub(super) fn search(arguments: &Json) -> ApiResult<Json> {
    let definition = definitions()
        .into_iter()
        .find(|tool| tool.name == "docs")
        .ok_or_else(|| ApiError::internal("the documentation tool declaration is absent"))?;
    orch_types::schema::validate(&definition.parameters, arguments)
        .map_err(|_| denied("The tool's arguments do not match its schema"))?;
    let words = arguments
        .get("query")
        .and_then(Json::as_str)
        .ok_or_else(|| denied("Missing query"))?
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut english = Vec::new();
    let mut chinese = Vec::new();
    for (name, source) in HELP {
        if let Some(hit) = hit(name, source, &words)? {
            if name.starts_with("zh-CN/") {
                chinese.push(hit);
            } else {
                english.push(hit);
            }
        }
    }
    let mut english = english.into_iter();
    let mut chinese = chinese.into_iter();
    let mut sources = Vec::new();
    while sources.len() < HITS {
        let mut found = false;
        for next in [english.next(), chinese.next()].into_iter().flatten() {
            found = true;
            if sources.len() < HITS {
                sources.push(next);
            }
        }
        if !found {
            break;
        }
    }
    let citations = sources
        .iter()
        .map(|hit| {
            json!({"url":hit["url"],
        "reference":hit["reference"],"title":hit["title"]})
        })
        .collect::<Vec<_>>();
    Ok(json!({"sources":sources,"citations":citations}))
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn canonical_headings_follow_front_matter_and_ignore_fenced_code() -> ApiResult<()> {
        let words = vec!["needle".to_owned()];
        let english = "---\nid: canonical-help\n---\n# Help\n```md\n## Needle decoy\n```\n## Real Needle ##\nRead this fact.\n";
        let chinese = "---\nid: canonical-help\nheading_id.真正的-needle: real-needle\n---\n# 帮助\n## 真正的 Needle\n阅读此事实。\n";
        let en = hit("help.md", english, &words)?;
        let zh = hit("zh-CN/help.md", chinese, &words)?;
        assert_eq!(
            en.as_ref().map(|hit| &hit["url"]),
            Some(&json!("/help/canonical-help#real-needle"))
        );
        assert_eq!(
            zh.as_ref().map(|hit| &hit["url"]),
            Some(&json!("/help/canonical-help#real-needle"))
        );
        assert_eq!(
            zh.as_ref().map(|hit| &hit["language"]),
            Some(&json!("zh-CN"))
        );
        assert!(
            en.as_ref()
                .and_then(|hit| hit["text"].as_str())
                .is_some_and(|text| !text.contains("decoy"))
        );
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_canonical_headings_follow_front_matter_and_ignore_fenced_code()
    -> crate::middleware::requests::ApiResult<()> {
        self::canonical_headings_follow_front_matter_and_ignore_fenced_code()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn public_documentation_returns_both_languages_and_rejects_extra_arguments() -> ApiResult<()> {
        let reply = search(&json!({"query":"ask.models"}))?;
        let sources = reply["sources"]
            .as_array()
            .ok_or_else(|| ApiError::internal("documentation has no sources"))?;
        assert!(sources.iter().any(|source| source["language"] == "en"));
        assert!(sources.iter().any(|source| source["language"] == "zh-CN"));
        assert!(sources.iter().all(|source| {
            source["url"]
                .as_str()
                .is_some_and(|url| url.starts_with("/help/"))
        }));
        assert!(search(&json!({"query":"Ask", "root_org_id":new_id()})).is_err());
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_public_documentation_returns_both_languages_and_rejects_extra_arguments()
    -> crate::middleware::requests::ApiResult<()> {
        self::public_documentation_returns_both_languages_and_rejects_extra_arguments()
    }
}
