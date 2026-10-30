//! Atomic directory-watch installation and rollback, including cancelled first reads.

use super::{DIR_WATCHES_MAX, DirWatch, Environment, lock, signature_of};
use crate::error::{HostError, Result};
use crate::files::{DirectoryLocation, Listing};
use orch_types::{Id, new_id};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A temporary native owner; retained by the actual capture even if its first caller goes away.
pub(super) struct Capture {
    environment: Arc<Environment>,
    location: DirectoryLocation,
}

impl Capture {
    pub(super) fn begin(
        environment: Arc<Environment>,
        location: DirectoryLocation,
    ) -> Result<Arc<Self>> {
        {
            let watcher = lock(&environment.watcher);
            let watcher = watcher.as_ref().ok_or_else(|| {
                HostError::Unsupported("the environment has no directory watcher".to_owned())
            })?;
            let watches = lock(&environment.watches);
            let mut captures = lock(&environment.directory_captures);
            if !registered(&watches, &captures, &location) {
                let path = location.path.under(environment.files.root().root_path());
                if held(&watches, &captures, &location.path) {
                    watcher.replace_watch_dir(&path)?;
                } else {
                    capacity(&watches, &captures)?;
                    watcher.try_watch_dir(&path)?;
                }
            } else if watcher.is_degraded() {
                return Err(HostError::Unsupported(
                    "native directory watching is unavailable".to_owned(),
                ));
            }
            if let Some((_, owners)) = captures.iter_mut().find(|(held, _)| *held == location) {
                *owners += 1;
            } else {
                captures.push((location.clone(), 1));
            }
        }
        Ok(Arc::new(Self {
            environment,
            location,
        }))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let watcher = lock(&self.environment.watcher);
        let watches = lock(&self.environment.watches);
        let mut captures = lock(&self.environment.directory_captures);
        if let Some(index) = captures.iter().position(|(held, _)| *held == self.location) {
            if captures[index].1 > 1 {
                captures[index].1 -= 1;
            } else {
                captures.swap_remove(index);
            }
        }
        if !held(&watches, &captures, &self.location.path)
            && let Some(watcher) = watcher.as_ref()
        {
            watcher.unwatch_dir(
                &self
                    .location
                    .path
                    .under(self.environment.files.root().root_path()),
            );
        }
    }
}

fn held(
    watches: &std::collections::HashMap<Id, DirWatch>,
    captures: &[(DirectoryLocation, usize)],
    dir: &crate::paths::RelPath,
) -> bool {
    watches.values().any(|watch| watch.dir == *dir)
        || captures.iter().any(|(capture, _)| capture.path == *dir)
}

fn registered(
    watches: &std::collections::HashMap<Id, DirWatch>,
    captures: &[(DirectoryLocation, usize)],
    location: &DirectoryLocation,
) -> bool {
    watches.values().any(|watch| watch.location == *location)
        || captures.iter().any(|(capture, _)| capture == location)
}

fn capacity(
    watches: &std::collections::HashMap<Id, DirWatch>,
    captures: &[(DirectoryLocation, usize)],
) -> Result<()> {
    // The same physical directory can have many logical readers. Temporary captures share those
    // native registrations and the existing bound; they never create an extra watch allowance.
    let directories: std::collections::BTreeSet<_> = watches
        .values()
        .map(|watch| &watch.dir)
        .chain(captures.iter().map(|(capture, _)| &capture.path))
        .collect();
    if directories.len() >= DIR_WATCHES_MAX {
        return Err(HostError::Busy(format!(
            "at most {DIR_WATCHES_MAX} directories are watched at once"
        )));
    }
    Ok(())
}

/// The first reset owns this exact installation until it has been published.
pub(super) struct Registration {
    environment: Arc<Environment>,
    watch: Id,
    instance: Id,
    previous: Option<DirWatch>,
    pub(super) location: DirectoryLocation,
    committed: bool,
}

impl Registration {
    pub(super) fn id(&self) -> Id {
        self.watch
    }

    pub(super) fn begin(
        environment: Arc<Environment>,
        watch: Id,
        wire_instance: Option<Id>,
        shown: crate::paths::RelPath,
        location: DirectoryLocation,
    ) -> Result<Self> {
        let instance = wire_instance.unwrap_or_else(new_id);
        let previous = {
            // Follow the background watcher's lock order; no lock spans an await.
            let watcher = lock(&environment.watcher);
            let watcher = watcher.as_ref().ok_or_else(|| {
                HostError::Unsupported("the environment has no directory watcher".to_owned())
            })?;
            let mut watches = lock(&environment.watches);
            if watches.len() >= DIR_WATCHES_MAX && !watches.contains_key(&watch) {
                return Err(HostError::Busy(format!(
                    "at most {DIR_WATCHES_MAX} directories are watched at once"
                )));
            }
            let captures = lock(&environment.directory_captures);
            // Refuse a failed native activation without replacing an existing logical reader.
            if !registered(&watches, &captures, &location) {
                let path = location.path.under(environment.files.root().root_path());
                let registered = if held(&watches, &captures, &location.path) {
                    watcher.replace_watch_dir(&path)
                } else {
                    capacity(&watches, &captures)?;
                    watcher.try_watch_dir(&path)
                };
                if let Err(error) = registered {
                    watcher.unwatch_dir(&location.path.under(environment.files.root().root_path()));
                    return Err(error);
                }
            } else if watcher.is_degraded() {
                return Err(HostError::Unsupported(
                    "native directory watching is unavailable".to_owned(),
                ));
            }
            watches.insert(
                watch,
                DirWatch {
                    instance,
                    wire_instance,
                    shown,
                    dir: location.path.clone(),
                    location: location.clone(),
                    last: BTreeMap::new(),
                    truncated: false,
                    invalidated: false,
                },
            )
        };
        Ok(Self {
            environment,
            watch,
            instance,
            previous,
            location,
            committed: false,
        })
    }

    pub(super) fn baseline(&self, listing: &Listing) -> Result<()> {
        let mut watches = lock(&self.environment.watches);
        let watch = watches
            .get_mut(&self.watch)
            .filter(|watch| watch.instance == self.instance)
            .ok_or_else(|| {
                HostError::Changed("the directory watch ended during its first read".to_owned())
            })?;
        watch.last = listing
            .entries
            .iter()
            .map(|entry| (entry.name.clone(), signature_of(entry)))
            .collect();
        watch.truncated = listing.truncated;
        watch.invalidated = false;
        Ok(())
    }

    pub(super) fn commit(&mut self) -> Result<()> {
        let watcher = lock(&self.environment.watcher);
        let watches = lock(&self.environment.watches);
        let captures = lock(&self.environment.directory_captures);
        if !watches
            .get(&self.watch)
            .is_some_and(|watch| watch.instance == self.instance)
        {
            return Err(HostError::Changed(
                "the directory watch ended before its first reset completed".to_owned(),
            ));
        }
        if let Some(previous) = &self.previous
            && previous.dir != self.location.path
            && !held(&watches, &captures, &previous.dir)
            && let Some(watcher) = watcher.as_ref()
        {
            watcher.unwatch_dir(
                &previous
                    .dir
                    .under(self.environment.files.root().root_path()),
            );
        }
        self.committed = true;
        self.previous = None;
        Ok(())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let watcher = lock(&self.environment.watcher);
        let mut watches = lock(&self.environment.watches);
        let captures = lock(&self.environment.directory_captures);
        let previous_dir = self.previous.as_ref().map(|previous| previous.dir.clone());
        if watches
            .get(&self.watch)
            .is_some_and(|watch| watch.instance == self.instance)
        {
            if let Some(previous) = self.previous.take() {
                watches.insert(self.watch, previous);
            } else {
                watches.remove(&self.watch);
            }
        }
        if !held(&watches, &captures, &self.location.path)
            && let Some(watcher) = watcher.as_ref()
        {
            watcher.unwatch_dir(
                &self
                    .location
                    .path
                    .under(self.environment.files.root().root_path()),
            );
        }
        if let Some(previous) = previous_dir
            && previous != self.location.path
            && !held(&watches, &captures, &previous)
            && let Some(watcher) = watcher.as_ref()
        {
            watcher.unwatch_dir(&previous.under(self.environment.files.root().root_path()));
        }
    }
}

/// Only the last logical watcher of an actual directory releases its OS watch.
pub(super) fn unwatch(environment: &Environment, id: Id) -> bool {
    let watcher = lock(&environment.watcher);
    let mut watches = lock(&environment.watches);
    let captures = lock(&environment.directory_captures);
    let Some(removed) = watches.remove(&id) else {
        return false;
    };
    if !held(&watches, &captures, &removed.dir)
        && let Some(watcher) = watcher.as_ref()
    {
        watcher.unwatch_dir(&removed.dir.under(environment.files.root().root_path()));
    }
    true
}
