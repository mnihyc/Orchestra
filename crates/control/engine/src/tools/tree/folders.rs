//! Typed document-tree presentation. Authority and mutations stay in the common document domain.

use super::*;

/// Live stored basenames are authoritative. Deleted entries and redacted structural ancestors
/// take no name from them, so a current path still resolves when an older twin can be restored.
pub(super) fn presentation_names(
    entries: &HashMap<organization::Item, organization::Listed>,
    ancestors: &HashMap<Id, organization::Ancestor>,
) -> HashMap<organization::Item, String> {
    let mut candidates: Vec<_> = entries
        .values()
        .map(|listed| {
            let entry = &listed.entry;
            (
                u8::from(entry.deleted_at.is_some()),
                entry.item,
                entry.parent,
                entry.basename.clone(),
            )
        })
        .collect();
    candidates.extend(
        ancestors
            .values()
            .filter(|ancestor| !entries.contains_key(&organization::Item::Container(ancestor.id)))
            .map(|ancestor| {
                let hidden = ancestor.basename.is_none();
                (
                    if hidden { 2 } else { 0 },
                    organization::Item::Container(ancestor.id),
                    ancestor.parent,
                    ancestor
                        .basename
                        .clone()
                        .unwrap_or_else(|| format!("_private-folder~{}", ancestor.id.simple())),
                )
            }),
    );
    candidates.sort_by_key(|(order, item, _, _)| (*order, item.id()));
    let mut taken: HashMap<organization::Directory, HashSet<String>> = HashMap::new();
    let mut names = HashMap::new();
    for (_, item, parent, basename) in candidates {
        let siblings = taken.entry(parent).or_default();
        let mut name = basename.clone();
        if !siblings.insert(name.clone()) {
            let extension = if matches!(item, organization::Item::Document(_)) {
                DOCUMENT_SUFFIX
            } else {
                ""
            };
            let stem = basename.strip_suffix(extension).unwrap_or(&basename);
            let digits = item.id().simple().to_string();
            name = (SUFFIX_CHARS..=digits.len())
                .map(|length| format!("{stem}~{}{extension}", &digits[digits.len() - length..]))
                .find(|candidate| siblings.insert(candidate.clone()))
                .unwrap_or_else(|| {
                    let mut suffix = format!("{stem}~{digits}");
                    while !siblings.insert(format!("{suffix}{extension}")) {
                        suffix.push('~');
                    }
                    format!("{suffix}{extension}")
                });
        }
        names.insert(item, name);
    }
    names
}

/// Exact basenames win over a case-insensitive convenience match.
pub(super) fn matching_basenames(names: &[String], part: &str) -> Vec<usize> {
    if let Some(at) = names.iter().position(|name| name == part) {
        return vec![at];
    }
    names
        .iter()
        .enumerate()
        .filter_map(|(at, name)| name.eq_ignore_ascii_case(part).then_some(at))
        .collect()
}

/// Keep the established ambiguity protection for readable live title aliases. Retired entries
/// participate only by their assigned name, so an old title cannot shadow a live document.
pub(super) fn matching_documents(documents: &[&DocEntry], part: &str) -> Vec<usize> {
    let live: Vec<_> = documents
        .iter()
        .enumerate()
        .filter(|(_, document)| document.row.deleted_at.is_none())
        .collect();
    let titles: Vec<_> = live
        .iter()
        .map(|(_, document)| (document.row.id, document.row.title.clone()))
        .collect();
    let names: Vec<_> = live
        .iter()
        .map(|(_, document)| document.name.clone())
        .collect();
    let mut found: Vec<_> = matching_names(&titles, &names, part, DOCUMENT_SUFFIX)
        .into_iter()
        .map(|at| live[at].0)
        .collect();
    let deleted: Vec<_> = documents
        .iter()
        .enumerate()
        .filter(|(_, document)| document.row.deleted_at.is_some())
        .collect();
    let names: Vec<_> = deleted
        .iter()
        .map(|(_, document)| document.name.clone())
        .collect();
    found.extend(
        matching_basenames(&names, part)
            .into_iter()
            .map(|at| deleted[at].0),
    );
    found
}

pub(super) fn read_error(error: organization::Error) -> EngineError {
    match error {
        organization::Error::Storage(error) => EngineError::Db(error),
        organization::Error::Authorization(error) => error.into(),
        organization::Error::Content(error) => error.into(),
        organization::Error::Engine(error) => error,
        _ => EngineError::unreadable("the document tree changed while it was being read"),
    }
}

pub(super) fn standing(current: organization::Standing, through: Standing) -> Standing {
    Standing {
        read: current.read.then_some(through.read.unwrap_or(Via::Share)),
        write: current.write.then_some(through.write.unwrap_or(Via::Share)),
        manage: current
            .manage
            .then_some(through.manage.unwrap_or(Via::Share)),
        shared_by: through.shared_by,
    }
}

impl Tree {
    /// Current typed membership read with the actual agent, never its accountable owner.
    pub(crate) fn organized_entry(
        &self,
        item: organization::Item,
    ) -> Option<&organization::Listed> {
        self.organized.get(&item)
    }

    /// The document's immediate directory, including real folders below the virtual docs root.
    pub(crate) fn document_parent(&self, document: Id) -> Option<organization::Directory> {
        self.organized
            .get(&organization::Item::Document(document))
            .map(|entry| entry.entry.parent)
            .or_else(|| {
                self.document_either(document)
                    .map(|entry| organization::Directory::NodeDocs(entry.row.node_id))
            })
    }

    /// The actual hierarchy anchor of a directory already present in this agent's tree.
    #[must_use]
    pub fn directory_node(&self, directory: organization::Directory) -> Option<Id> {
        match directory {
            organization::Directory::NodeDocs(node) => Some(node),
            organization::Directory::Container(id) => self
                .organized
                .get(&organization::Item::Container(id))
                .map(|entry| entry.entry.node)
                .or_else(|| self.folder_ancestors.get(&id).map(|entry| entry.node)),
        }
    }

    /// Containing document folders, nearest first, from the loaded structural path. A person
    /// projecting this agent's tree must check these names independently before exposing a path.
    #[must_use]
    pub fn document_folder_ancestors(&self, item: Id) -> Vec<Id> {
        let mut parent = if self.folder_exists(item) {
            self.folder_parent(item)
        } else {
            self.document_parent(item)
        };
        let mut ancestors = Vec::new();
        let mut seen = HashSet::new();
        while let Some(organization::Directory::Container(id)) = parent {
            if !seen.insert(id) {
                break;
            }
            ancestors.push(id);
            parent = self.folder_parent(id);
        }
        ancestors
    }

    pub(crate) fn folder_standing(&self, id: Id) -> organization::Standing {
        self.organized
            .get(&organization::Item::Container(id))
            .map(|entry| entry.standing)
            .or_else(|| self.folder_ancestors.get(&id).map(|entry| entry.standing))
            .unwrap_or_default()
    }

    fn folder_parent(&self, id: Id) -> Option<organization::Directory> {
        self.organized
            .get(&organization::Item::Container(id))
            .map(|entry| entry.entry.parent)
            .or_else(|| self.folder_ancestors.get(&id).map(|entry| entry.parent))
    }

    fn folder_name(&self, id: Id) -> Option<String> {
        self.document_names
            .get(&organization::Item::Container(id))
            .cloned()
    }

    /// Canonical directory paths preserve folder ancestry; unreadable names stay redacted.
    pub(crate) fn path_of_directory(&self, directory: organization::Directory) -> Option<String> {
        let mut names: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        let mut parent = directory;
        loop {
            match parent {
                organization::Directory::NodeDocs(node) => {
                    let mut path = self.docs_path_of(node)?;
                    for name in names.into_iter().rev() {
                        path.push_str(&name);
                        path.push('/');
                    }
                    return Some(path);
                }
                organization::Directory::Container(id) => {
                    if !seen.insert(id) {
                        return None;
                    }
                    names.push(self.folder_name(id)?);
                    parent = self.folder_parent(id)?;
                }
            }
        }
    }

    pub(crate) fn document_path(&self, document: Id) -> Option<String> {
        let entry = self.document_either(document)?;
        Some(format!(
            "{}{}",
            self.path_of_directory(self.document_parent(document)?)?,
            entry.name
        ))
    }

    pub(super) fn folder_children(&self, parent: organization::Directory) -> Vec<Id> {
        let mut children: HashSet<_> = self
            .organized
            .values()
            .filter_map(|entry| match entry.entry.item {
                organization::Item::Container(id) if entry.entry.parent == parent => Some(id),
                _ => None,
            })
            .collect();
        children.extend(
            self.folder_ancestors
                .values()
                .filter(|entry| entry.parent == parent)
                .map(|entry| entry.id),
        );
        let mut children: Vec<_> = children.into_iter().collect();
        children.sort_by_key(|id| self.folder_name(*id));
        children
    }

    pub(super) fn folder_entry(&self, id: Id, prefix: &str) -> Option<Entry> {
        let name = self.folder_name(id)?;
        let mut entry = Entry::bare(
            path_of(prefix, &name, true),
            name,
            SharedEntryKind::Folder,
            id,
            Where::DocumentFolder(id),
        );
        let standing = self.folder_standing(id);
        let deleted = self
            .organized
            .get(&organization::Item::Container(id))
            .is_some_and(|entry| entry.entry.deleted_at.is_some());
        let archived = self
            .directory_node(organization::Directory::Container(id))
            .is_some_and(|node| self.reach.archived(node));
        entry.deleted = deleted;
        entry.readable = standing.read && !deleted;
        entry.writable = standing.write && !deleted && !archived;
        entry.manageable = standing.manage && !archived;
        // Count only entries this agent may see; hidden children never become a disclosed headcount.
        if standing.read {
            entry.children = Some(
                i64::try_from(
                    self.organized
                        .values()
                        .filter(|item| item.entry.parent == organization::Directory::Container(id))
                        .count(),
                )
                .unwrap_or(i64::MAX),
            );
        }
        Some(entry)
    }

    pub(super) fn folder_exists(&self, id: Id) -> bool {
        self.organized
            .contains_key(&organization::Item::Container(id))
            || self.folder_ancestors.contains_key(&id)
    }
}
