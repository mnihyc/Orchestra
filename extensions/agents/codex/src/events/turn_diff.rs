//! Turn summaries are snapshots of reported patches, never file contents or tool executions.
use super::*;

fn quoted_path(value: &str) -> Option<(String, usize)> {
    let bytes = value.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut path = Vec::new();
    let mut at = 1;
    while let Some(&byte) = bytes.get(at) {
        at += 1;
        match byte {
            b'"' => return String::from_utf8(path).ok().map(|path| (path, at)),
            b'\\' => {
                let escaped = *bytes.get(at)?;
                at += 1;
                path.push(match escaped {
                    b'a' => 7,
                    b'b' => 8,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 11,
                    b'f' => 12,
                    b'r' => b'\r',
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'0'..=b'7' => {
                        let mut octal = u16::from(escaped - b'0');
                        for _ in 0..2 {
                            let digit = *bytes.get(at)?;
                            if !(b'0'..=b'7').contains(&digit) {
                                return None;
                            }
                            octal = octal * 8 + u16::from(digit - b'0');
                            at += 1;
                        }
                        u8::try_from(octal).ok()?
                    }
                    _ => return None,
                });
            }
            byte => path.push(byte),
        }
    }
    None
}

fn path(value: &str) -> Option<String> {
    let value = value.trim_end_matches(['\r', '\n']);
    if value.starts_with('"') {
        quoted_path(value).map(|(path, _)| path)
    } else {
        Some(value.split('\t').next()?.to_owned())
    }
}

fn git_paths(value: &str) -> Option<(String, String)> {
    // Unquoted identical paths can contain the separator too; their equal byte lengths locate it.
    let middle = value.len() / 2;
    if value.as_bytes().get(middle) == Some(&b' ')
        && let (Some(before), Some(after)) = (value.get(..middle), value.get(middle + 1..))
        && before
            .strip_prefix("a/")
            .zip(after.strip_prefix("b/"))
            .is_some_and(|(old, new)| old == new)
    {
        return Some((before.to_owned(), after.to_owned()));
    }
    if value.starts_with('"') {
        let (before, end) = quoted_path(value)?;
        Some((before, path(value.get(end..)?.trim_start())?))
    } else if let Some((before, after)) = value.split_once(" \"") {
        Some((before.to_owned(), path(&format!("\"{after}"))?))
    } else {
        let (before, after) = value.rsplit_once(" b/")?;
        Some((before.to_owned(), format!("b/{after}")))
    }
}

fn hunk_counts(line: &str) -> Option<(usize, usize)> {
    let mut fields = line.strip_prefix("@@ ")?.split_whitespace();
    let count = |field: &str, prefix| {
        let range = field.strip_prefix(prefix)?;
        range
            .split_once(',')
            .map_or(Some(1), |(_, count)| count.parse().ok())
    };
    Some((count(fields.next()?, '-')?, count(fields.next()?, '+')?))
}

fn change(block: &str, directory: &str) -> Option<FileChange> {
    let mut before = None;
    let mut after = None;
    let mut rename = false;
    let mut binary = false;
    let mut hunk = false;
    let mut added = 0;
    let mut removed = 0;
    for line in block.lines() {
        if line.starts_with("@@ ") {
            hunk = true;
        }
        if hunk {
            if line.starts_with('+') {
                added += 1;
            } else if line.starts_with('-') {
                removed += 1;
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("diff --git ") {
            if let Some((old, new)) = git_paths(value) {
                before = Some(old.strip_prefix("a/").unwrap_or(&old).to_owned());
                after = Some(new.strip_prefix("b/").unwrap_or(&new).to_owned());
            }
        } else if let Some(value) = line.strip_prefix("--- ") {
            let old = path(value)?;
            before = Some(old.strip_prefix("a/").unwrap_or(&old).to_owned());
        } else if let Some(value) = line.strip_prefix("+++ ") {
            let new = path(value)?;
            after = Some(new.strip_prefix("b/").unwrap_or(&new).to_owned());
        } else if let Some(value) = line.strip_prefix("rename from ") {
            before = Some(path(value)?);
            rename = true;
        } else if let Some(value) = line.strip_prefix("rename to ") {
            after = Some(path(value)?);
            rename = true;
        } else if line.starts_with("copy from ") {
            before = Some("/dev/null".into());
        } else if let Some(value) = line.strip_prefix("copy to ") {
            after = Some(path(value)?);
        } else if line.starts_with("new file mode ") {
            before = Some("/dev/null".into());
        } else if line.starts_with("deleted file mode ") {
            after = Some("/dev/null".into());
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            binary = true;
        }
    }
    let (before, after) = (before?, after?);
    let kind = if before == "/dev/null" {
        FileChangeKind::Add
    } else if after == "/dev/null" {
        FileChangeKind::Delete
    } else if rename || before != after {
        FileChangeKind::Rename
    } else {
        FileChangeKind::Modify
    };
    let name = if kind == FileChangeKind::Delete {
        &before
    } else {
        &after
    };
    if name.is_empty() || name == "/dev/null" {
        return None;
    }
    let (diff, truncated) = cut_diff(block.to_owned());
    Some(FileChange {
        before: None,
        after: None,
        path: relative_to(name, directory),
        change: kind,
        old_path: (kind == FileChangeKind::Rename).then(|| relative_to(&before, directory)),
        diff: (!binary).then_some(diff),
        added,
        removed,
        binary,
        truncated,
    })
}

/// Split only outside hunks: removed and added text may itself look like a pair of file headers.
pub(super) fn changes(diff: &str, directory: &str) -> Option<Vec<FileChange>> {
    if diff.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut changes = Vec::new();
    let mut block = String::new();
    let mut paths = false;
    let mut remaining = (0_usize, 0_usize);
    for line in diff.split_inclusive('\n') {
        let in_hunk = remaining != (0, 0);
        if in_hunk && line.starts_with("diff --git ") {
            return None;
        }
        if !in_hunk
            && (line.starts_with("diff --git ") || paths && line.starts_with("--- "))
            && !block.is_empty()
        {
            changes.push(change(&block, directory)?);
            block.clear();
            paths = false;
        }
        block.push_str(line);
        if in_hunk {
            match line.as_bytes().first() {
                Some(b'-') => remaining.0 = remaining.0.saturating_sub(1),
                Some(b'+') => remaining.1 = remaining.1.saturating_sub(1),
                Some(b' ') => {
                    remaining.0 = remaining.0.saturating_sub(1);
                    remaining.1 = remaining.1.saturating_sub(1);
                }
                _ => {}
            }
        } else if line.starts_with("@@ ") {
            remaining = hunk_counts(line)?;
        } else if line.starts_with("+++ ") {
            paths = true;
        }
    }
    if remaining != (0, 0) {
        return None;
    }
    if !block.is_empty() {
        changes.push(change(&block, directory)?);
    }
    Some(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_paths_keep_embedded_separators_and_trailing_spaces_and_copies_are_additions() {
        let files = changes(concat!(
            "diff --git a/dir b/pixel.png  b/dir b/pixel.png \nBinary files a/dir b/pixel.png  and b/dir b/pixel.png  differ\n",
            "diff --git a/original b/copied\nsimilarity index 100%\ncopy from original\ncopy to copied\n"
        ), "/workspace").unwrap();
        assert_eq!(files[0].path, "dir b/pixel.png ");
        assert!(files[0].binary);
        assert_eq!(files[1].path, "copied");
        assert_eq!(files[1].change, FileChangeKind::Add);
        let plain = changes(
            "diff --git source target\n--- source\n+++ target\n@@ -1 +1 @@\n-old\n+new\n",
            "/workspace",
        )
        .unwrap();
        assert_eq!(plain[0].path, "target");
    }

    #[test]
    fn reported_patches_keep_added_deleted_renamed_binary_and_quoted_paths() {
        let diff = concat!(
            "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+new\n",
            "diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n",
            "diff --git a/old name b/new name\nsimilarity index 100%\nrename from old name\nrename to new name\n",
            "diff --git a/pixel.png b/pixel.png\nBinary files a/pixel.png and b/pixel.png differ\n",
            "diff --git \"a/\\346\\226\\207.txt\" \"b/\\346\\226\\207.txt\"\n--- \"a/\\346\\226\\207.txt\"\n+++ \"b/\\346\\226\\207.txt\"\n@@ -1 +1 @@\n-old\n+new\n"
        );
        let files = changes(diff, "/workspace").unwrap();
        assert_eq!(files.len(), 5);
        assert_eq!(files[0].change, FileChangeKind::Add);
        assert_eq!(files[0].added, 1);
        assert_eq!(files[1].change, FileChangeKind::Delete);
        assert_eq!(files[1].removed, 1);
        assert_eq!(files[2].change, FileChangeKind::Rename);
        assert_eq!(files[2].old_path.as_deref(), Some("old name"));
        assert_eq!(files[2].path, "new name");
        assert!(files[3].binary);
        assert!(files[3].diff.is_none());
        assert_eq!(files[4].path, "文.txt");
        assert!(
            files
                .iter()
                .all(|file| file.before.is_none() && file.after.is_none())
        );
    }

    #[test]
    fn ordinary_unified_patches_do_not_split_at_header_looking_content() {
        let files = changes("--- a/first\n+++ b/first\n@@ -1 +1 @@\n--- old\n+++ new\n--- a/second\n+++ b/second\n@@ -0,0 +1 @@\n+text\n", "/workspace").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "first");
        assert_eq!((files[0].added, files[0].removed), (1, 1));
        assert_eq!(files[1].path, "second");
        assert!(changes("unrecognized patch", "/workspace").is_none());
    }
}
