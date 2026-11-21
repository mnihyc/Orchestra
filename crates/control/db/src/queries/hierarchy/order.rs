//! Native hierarchy order and captured visible keyset progress.
use super::TreeRow;
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};

pub const KIND: &str =
    "CASE n.kind WHEN 'org' THEN 0 WHEN 'project' THEN 1 WHEN 'topic' THEN 2 ELSE 3 END";
pub const NODE_ACTIVITY: &str = "GREATEST(n.activity_at,n.updated_at,n.created_at)";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum By {
    #[default]
    Id,
    Newest,
    Changed,
    Name,
    Custom,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    En,
    ZhCn,
}
impl Locale {
    pub const fn collation(self) -> &'static str {
        match self {
            Self::En => "orch_navigation_en",
            Self::ZhCn => "orch_navigation_zh",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Order {
    pub by: By,
    pub locale: Locale,
    pub custom: Json,
    pub thread_activity: bool,
}
impl Default for Order {
    fn default() -> Self {
        Self {
            by: By::Id,
            locale: Locale::En,
            custom: Json::Object(Default::default()),
            thread_activity: false,
        }
    }
}

/// Only already admitted fields enter a public cursor. Inspection progress never carries work facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub id: Id,
    pub kind: String,
    pub parent: Option<Id>,
    pub name: String,
    pub activity: Option<DateTime<Utc>>,
    pub seq: i64,
    pub inspection: bool,
}
impl Progress {
    pub fn identity(id: Id) -> Self {
        Self {
            id,
            kind: String::new(),
            parent: None,
            name: String::new(),
            activity: None,
            seq: 0,
            inspection: false,
        }
    }
    pub fn row(row: &TreeRow, by: By, inspection: bool) -> Self {
        Self {
            id: row.id,
            kind: row.kind.clone(),
            parent: row.parent_id,
            name: if matches!(by, By::Name | By::Changed) || inspection {
                row.name.clone()
            } else {
                String::new()
            },
            activity: if by == By::Changed && !inspection {
                Some(row.activity_at)
            } else {
                None
            },
            seq: if by == By::Changed && !inspection {
                row.tree_seq
            } else {
                0
            },
            inspection,
        }
    }
    pub fn kind_rank(&self) -> i32 {
        match self.kind.as_str() {
            "org" => 0,
            "project" => 1,
            "topic" => 2,
            _ => 3,
        }
    }
}

impl Order {
    pub fn rank(&self, parent: Option<Id>, id: Id) -> i64 {
        let key = id.to_string();
        parent
            .and_then(|parent| self.custom.get(parent.to_string().as_str()))
            .and_then(Json::as_array)
            .and_then(|ids| {
                ids.iter()
                    .position(|value| value.as_str() == Some(key.as_str()))
            })
            .and_then(|at| i64::try_from(at + 1).ok())
            .unwrap_or(i64::MAX)
    }
    pub fn activity(&self) -> &'static str {
        if self.thread_activity {
            "GREATEST(n.activity_at,n.updated_at,n.created_at,ot.last_activity_at)"
        } else {
            NODE_ACTIVITY
        }
    }
    pub fn joins(&self) -> String {
        let mut joins = String::from(" CROSS JOIN progress q ");
        if self.thread_activity && self.by == By::Changed {
            joins.push_str(" LEFT JOIN threads ot ON ot.node_id=n.id ");
        }
        if self.by == By::Custom {
            joins.push_str(" LEFT JOIN custom_order co ON co.parent=n.parent_id AND co.id=n.id ");
        }
        joins
    }
    pub fn ordering(&self) -> String {
        let name = format!("n.name COLLATE {}", self.locale.collation());
        match self.by {
            By::Id => "n.id".to_owned(),
            By::Newest => format!("{KIND},n.id DESC"),
            By::Name => format!("{KIND},{name},n.id"),
            By::Changed => format!(
                "{KIND},{} DESC,n.tree_seq DESC,{name},n.id",
                self.activity()
            ),
            By::Custom => {
                "n.parent_id NULLS FIRST,COALESCE(co.rank,9223372036854775807),n.id DESC".to_owned()
            }
        }
    }
    pub fn after(&self) -> String {
        let name = format!("n.name COLLATE {}", self.locale.collation());
        let prior = format!("q.name COLLATE {}", self.locale.collation());
        match self.by {
            By::Id=>"n.id>$4".to_owned(),
            By::Newest=>format!("({KIND}>q.kind OR ({KIND}=q.kind AND n.id<$4))"),
            By::Name=>format!("({KIND},{name},n.id)>(q.kind,{prior},$4)"),
            By::Changed=>{
                let at=self.activity();
                format!("({KIND}>q.kind OR ({KIND}=q.kind AND ({at}<q.stamp OR ({at}=q.stamp AND (n.tree_seq<q.seq OR (n.tree_seq=q.seq AND ({name},n.id)>({prior},$4)))))))")
            }
            By::Custom=>"(COALESCE(n.parent_id,'00000000-0000-0000-0000-000000000000'::uuid)>COALESCE(q.parent,'00000000-0000-0000-0000-000000000000'::uuid) OR (n.parent_id IS NOT DISTINCT FROM q.parent AND (COALESCE(co.rank,9223372036854775807)>q.rank OR (COALESCE(co.rank,9223372036854775807)=q.rank AND n.id<$4))))".to_owned(),
        }
    }
    /// Max activity's first window lies in the union of each timestamp stream's first window.
    /// Each raw stream is bounded before native View admission; no full sibling vector is built.
    pub fn candidates(&self, scope: &str, has_parent: bool) -> String {
        let joins = self.joins();
        let after = self.after();
        let scope = scope.replace("__AFTER__", &after);
        let order = self.ordering();
        if self.by != By::Changed || !self.thread_activity {
            return format!(
                "SELECT n.* FROM nodes n {joins} WHERE {scope} ORDER BY {order} LIMIT $3"
            );
        }
        let name = format!("n.name COLLATE {}", self.locale.collation());
        let first = format!(
            "SELECT n.id FROM nodes n {joins} WHERE {scope} ORDER BY {KIND},{NODE_ACTIVITY} DESC,n.tree_seq DESC,{name},n.id LIMIT $3"
        );
        let parent = if has_parent {
            " AND ot.topic_id=$5"
        } else {
            ""
        };
        let second = format!(
            "SELECT n.id FROM nodes n {joins} WHERE {scope} AND n.kind='thread' AND ot.last_activity_at IS NOT NULL {parent} ORDER BY ot.last_activity_at DESC,n.tree_seq DESC,{name},n.id LIMIT $3"
        );
        format!(
            "SELECT n.* FROM nodes n {joins} JOIN (({first}) UNION ({second})) chosen ON chosen.id=n.id ORDER BY {order} LIMIT $3"
        )
    }
}
