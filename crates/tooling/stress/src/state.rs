//! What the seeder made, kept in a JSON file between runs: a later `seed` adds only what is missing (a larger
//! `--scale` grows the same world), and `drive` reads what to drive from it.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One of the people the drive acts as.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonRec {
    pub login: String,
    pub id: String,
    /// The orgs they are a member of (indexes into `orgs`).
    pub orgs: Vec<usize>,
}

/// An org with what its threads run on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgRec {
    pub id: String,
    pub slug: String,
    /// Its environment on the agent machine, when it could be made.
    pub environment: Option<String>,
    pub providers: Vec<String>,
    pub agents: Vec<String>,
    /// Its members among the people (indexes into `people`).
    pub members: Vec<usize>,
}

/// A project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRec {
    pub id: String,
    pub name: String,
    pub org: usize,
}

/// A topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicRec {
    pub id: String,
    pub name: String,
    pub project: usize,
    pub org: usize,
}

/// A thread, with the person who works in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRec {
    pub id: String,
    pub name: String,
    pub topic: usize,
    pub org: usize,
    /// Index into `people`; `None` when the seeding administrator made it and works in it.
    pub owner: Option<usize>,
    /// The provider it is bound to.
    #[serde(default)]
    pub provider: Option<String>,
}

/// A document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocRec {
    pub id: String,
    pub title: String,
    pub project: usize,
    pub org: usize,
    pub versions: u32,
    pub discussions: u32,
}

/// Everything made.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub api: String,
    pub prefix: String,
    /// The machine the threads run on (id, name).
    pub machine: Option<(String, String)>,
    /// The environment whose directory holds the orgs' environments' directories.
    pub carrier: Option<String>,
    pub people: Vec<PersonRec>,
    pub orgs: Vec<OrgRec>,
    pub projects: Vec<ProjectRec>,
    pub topics: Vec<TopicRec>,
    pub threads: Vec<ThreadRec>,
    pub docs: Vec<DocRec>,
    /// Hosts the system had when last seeded, by name.
    pub hosts: Vec<String>,
}

impl State {
    /// The state at `path`, or an empty one when there is no file yet.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
    }

    /// Writes the state to `path` (whole, through a file beside it, so that an interrupted write leaves the old one).
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        let partial = path.with_extension("json.partial");
        std::fs::write(&partial, text).with_context(|| format!("writing {}", partial.display()))?;
        std::fs::rename(&partial, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}
