//! Bounded workspace selections shared by file-copy plans and cached manifests.

use crate::EngineError;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use orch_link_contract::commands::{Selection, validate_selection_pattern};

/// Compiled include and exclude patterns, anchored at the workspace root.
pub struct Matcher {
    include: GlobSet,
    exclude: GlobSet,
}

fn compile(patterns: &[String], all_if_empty: bool) -> Result<GlobSet, EngineError> {
    let mut set = GlobSetBuilder::new();
    if patterns.is_empty() && all_if_empty {
        set.add(
            GlobBuilder::new("**")
                .literal_separator(true)
                .build()
                .map_err(EngineError::unreadable)?,
        );
    }
    for pattern in patterns {
        validate_selection_pattern(pattern).map_err(EngineError::unreadable)?;
        let pattern = if pattern.is_empty() {
            "**"
        } else {
            pattern.as_str()
        };
        // Only * and ? have pattern meaning; the wire grammar makes brackets and braces literal.
        let mut escaped = String::new();
        for character in pattern.chars() {
            if matches!(character, '[' | ']' | '{' | '}') {
                escaped.push('\\');
            }
            escaped.push(character);
        }
        set.add(
            GlobBuilder::new(&escaped)
                .literal_separator(true)
                .build()
                .map_err(EngineError::unreadable)?,
        );
        if !pattern.contains(['*', '?']) {
            set.add(
                GlobBuilder::new(&format!("{escaped}/**"))
                    .literal_separator(true)
                    .build()
                    .map_err(EngineError::unreadable)?,
            );
        }
    }
    set.build().map_err(EngineError::unreadable)
}

impl Matcher {
    /// Compiles each pattern once, without reading any file.
    pub fn new(selection: &Selection) -> Result<Self, EngineError> {
        Ok(Self {
            include: compile(&selection.include, true)?,
            exclude: compile(&selection.exclude, false)?,
        })
    }

    /// Whether one relative path is in the promised copy.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        self.include.is_match(path) && !self.exclude.is_match(path)
    }
}

/// Keeps a selected manifest's paths relative to the workspace or thread directory promised by the plan.
/// A connector may choose a narrower common root; this adds that prefix back without copying any content.
pub async fn rebase_manifest(
    engine: &crate::Engine,
    conn: &mut orch_db::Conn,
    blob: &orch_types::Hash,
    root: &str,
) -> Result<orch_types::Hash, EngineError> {
    let manifest = engine.read_manifest(blob).await?;
    let from = manifest.header().root.replace('\\', "/");
    let to = root.replace('\\', "/");
    if from == to {
        return Ok(*blob);
    }
    let prefix = from
        .strip_prefix(&format!("{}/", to.trim_end_matches('/')))
        .ok_or_else(|| {
            EngineError::unreadable("the selected manifest leaves the promised source directory")
        })?;
    let entries = manifest
        .entries()
        .iter()
        .cloned()
        .map(|mut entry| {
            entry.path = format!("{prefix}/{}", entry.path);
            entry
        })
        .collect();
    let mut header = manifest.header().clone();
    header.root = root.to_owned();
    let written = crate::manifest::build(&header, entries).map_err(EngineError::unreadable)?;
    Ok(engine.put_manifest(conn, written.bytes).await?.hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_copies_only_its_named_tree_and_literals_and_leaves_excluded_paths_out() {
        let matcher = Matcher::new(&Selection {
            include: vec!["src".into(), "data[1].csv".into()],
            exclude: vec!["src/cache".into()],
        })
        .unwrap();
        assert!(matcher.matches("src/main.rs"));
        assert!(matcher.matches("data[1].csv"));
        assert!(!matcher.matches("data1.csv") && !matcher.matches("other/main.rs"));
        assert!(!matcher.matches("src/cache/object"));
        assert!(
            Matcher::new(&Selection {
                include: vec!["../other".into()],
                exclude: Vec::new()
            })
            .is_err()
        );
    }
}
