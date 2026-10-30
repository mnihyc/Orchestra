//! Native field projections keep their recency and text indexes visible to the candidate planner.

/// The bounded sample and exact candidate entrance chosen by the caller.
pub(super) struct ReadPlan {
    pub ordered: bool,
    pub filtered_source: bool,
    pub recent_sample: bool,
}

/// One native field and the live rows whose expressions it exposes.
pub(super) struct Source {
    from: &'static str,
    live: &'static str,
    kind: &'static str,
    source: &'static str,
    part: &'static str,
    node: &'static str,
    block: &'static str,
    tier: &'static str,
    text: &'static str,
    at: &'static str,
    author: &'static str,
    item: &'static str,
    file: &'static str,
    mentions: &'static str,
}

impl Source {
    /// A field's native source exposes its metadata directly to the planner.
    pub(super) fn advanced_filter(&self, selection: &str, field: &str) -> String {
        let (owner, creator, editor, created, edited) = match field {
            "name" | "node_description" => (
                "entry.owner_id",
                "entry.created_by",
                "NULL::uuid",
                "entry.created_at",
                "entry.updated_at",
            ),
            "chat" => (
                "NULL::uuid",
                "c.created_by",
                "m.author_id",
                "m.created_at",
                "COALESCE(m.edited_at,m.created_at)",
            ),
            "item" | "revision" => (
                "n.owner_id",
                "n.created_by",
                "NULL::uuid",
                "n.created_at",
                "n.updated_at",
            ),
            _ => (
                "d.owner_id",
                "d.created_by",
                "d.updated_by",
                "d.created_at",
                "d.updated_at",
            ),
        };
        super::super::advanced_facts_sql(selection, owner, creator, editor, created, edited)
    }

    /// Check the file carried by this exact native version before bounding the stream.
    pub(super) fn file_filter(&self, selection: &str, field: &str) -> String {
        match field {
            "file" => super::super::file_type_sql(selection, "f.media_type"),
            "item" => super::super::file_json_sql(selection, "i.payload"),
            "revision" => super::super::file_json_sql(selection, "r.body->'payload'"),
            "chat" => super::super::file_json_sql(selection, "m.content"),
            "name" | "node_description" => "false".to_owned(),
            _ => format!(
                "d.search_file_count>0 AND EXISTS (SELECT 1 FROM document_files adv_f WHERE adv_f.document_id=d.id AND {})",
                super::super::file_type_sql(selection, "adv_f.media_type")
            ),
        }
    }

    /// A direct source for the fields whose ordered entrance otherwise hides behind a union view.
    pub(super) fn of(field: &str) -> Option<Self> {
        let mut row = Self {
            from: "documents d",
            live: "d.deleted_at IS NULL",
            kind: "'document'::text",
            source: "d.id",
            part: "d.id",
            node: "d.node_id",
            block: "NULL::uuid",
            tier: "4::int",
            text: "d.title",
            at: "d.updated_at",
            author: "d.updated_by",
            item: "NULL::text",
            file: "d.search_file_count>0",
            mentions: "NULL::jsonb",
        };
        match field {
            "title" => {}
            "name" | "node_description" => {
                row.from = "nodes entry";
                row.live = if field == "name" {
                    "entry.deleted_at IS NULL"
                } else {
                    "entry.deleted_at IS NULL AND entry.description<>''"
                };
                row.kind = "'name'::text";
                row.source = "entry.id";
                row.part = "entry.id";
                row.node = "entry.id";
                row.text = if field == "name" {
                    "entry.name"
                } else {
                    "entry.description"
                };
                row.tier = if field == "name" { "4::int" } else { "3::int" };
                row.at = "GREATEST(entry.updated_at,entry.activity_at)";
                row.author = "NULL::uuid";
                row.file = "false";
            }
            "description" => {
                row.live = "d.deleted_at IS NULL AND d.description<>''";
                row.tier = "3::int";
                row.text = "d.description";
            }
            "file" => {
                row.from = "document_files f JOIN documents d ON d.id=f.document_id";
                row.tier = "2::int";
                row.text = "f.name";
                row.at = "f.added_at";
                row.author = "f.added_by";
                row.file = "true";
            }
            "heading" | "block" => {
                row.from = "document_blocks b CROSS JOIN LATERAL (SELECT * FROM documents d WHERE d.id=b.document_id OFFSET 0)d";
                row.live = if field == "heading" {
                    "b.removed_at IS NULL AND d.deleted_at IS NULL AND b.type='heading'"
                } else {
                    "b.removed_at IS NULL AND d.deleted_at IS NULL AND b.type<>'heading'"
                };
                row.part = "b.id";
                row.block = "b.id";
                row.tier = if field == "heading" {
                    "2::int"
                } else {
                    "0::int"
                };
                row.text = "b.text";
                row.at = "b.updated_at";
                row.author = "b.updated_by";
                row.mentions = "b.content";
            }
            "chat" => {
                row.from = "chat_messages m CROSS JOIN LATERAL (SELECT * FROM chats c WHERE c.id=m.chat_id OFFSET 0)c";
                row.live = "m.deleted_at IS NULL AND m.search_text IS NOT NULL";
                row.kind = "'chat_message'::text";
                row.source = "m.chat_id";
                row.part = "m.id";
                row.node = "COALESCE(c.node_id,c.org_id)";
                row.tier = "0::int";
                row.text = "m.search_text";
                row.at = "m.created_at";
                row.author = "m.author_id";
                row.file = "orch_search_file_types(m.content) @> ARRAY['*/*']";
                row.mentions = "to_jsonb(m.mentions)";
            }
            "item" | "revision" => {
                row.from = if field == "item" {
                    "items i"
                } else {
                    "items i JOIN thread_item_revisions r ON r.item_id=i.id"
                };
                row.live = if field == "item" {
                    "i.search_text IS NOT NULL AND i.kind<>'marker' AND i.search_text NOT LIKE '%<orch>%'"
                } else {
                    "r.body->>'search_text' IS NOT NULL AND i.kind<>'marker' AND r.body->>'search_text' NOT LIKE '%<orch>%'"
                };
                row.kind = "'item'::text";
                row.source = "i.thread_id";
                row.part = "i.id";
                row.node = "i.thread_id";
                row.tier = "0::int";
                row.text = if field == "item" {
                    "i.search_text"
                } else {
                    "(r.body->>'search_text')"
                };
                row.at = "i.created_at";
                row.author = "i.author_id";
                row.item = "i.kind";
                row.file = if field == "item" {
                    "orch_search_file_types(i.payload) @> ARRAY['*/*']"
                } else {
                    "orch_search_file_types(r.body->'payload') @> ARRAY['*/*']"
                };
                row.mentions = if field == "item" {
                    "i.payload"
                } else {
                    "(r.body->'payload')"
                };
            }
            _ => return None,
        }
        Some(row)
    }

    /// Expands the shared filter directly against native columns, preserving every access and focus predicate.
    pub(super) fn render(
        &self,
        sql: String,
        field: &str,
        access: &str,
        scope: Option<&super::super::sparse::Scope>,
        plan: ReadPlan,
    ) -> String {
        let ReadPlan {
            ordered,
            filtered_source,
            recent_sample,
        } = plan;
        let mut from = self.from.to_owned();
        let mut live = self.live.to_owned();
        if field == "revision" && recent_sample {
            // This density sample bounds both recent items and versions per item. The indexed
            // text/MIME probe remains responsible for older matches when the sample is sparse.
            from = include_str!("../../search_revision_sample.sql").to_owned();
        }
        if filtered_source {
            // Let selective source metadata enter before its blocks or messages. The unfiltered
            // recency path keeps its existing barrier and deferred-text entrance.
            if matches!(field, "heading" | "block") {
                from = "document_blocks b JOIN documents d ON d.id=b.document_id".to_owned();
            } else if field == "chat" {
                from = "chat_messages m JOIN chats c ON c.id=m.chat_id".to_owned();
            }
        }
        if scope.is_some_and(|scope| scope.small) {
            // A recent scoped sample starts at its places. A text probe leaves both indexed join
            // orders available, so a rare word does not have to visit every document in the scope.
            if matches!(field, "heading" | "block") {
                from = if ordered {
                    let kind = if field == "heading" {
                        "b.type='heading'"
                    } else {
                        "b.type<>'heading'"
                    };
                    format!(
                        "documents d CROSS JOIN LATERAL (SELECT * FROM document_blocks b
                      WHERE b.document_id=d.id AND b.removed_at IS NULL AND {kind} OFFSET 0)b"
                    )
                } else {
                    "document_blocks b JOIN documents d ON d.id=b.document_id".to_owned()
                };
                if ordered {
                    // The inner entrance already enforces block liveness/type. Returning removed_at
                    // only to check it again outside the barrier would force a heap read per block.
                    live = "d.deleted_at IS NULL".to_owned();
                }
            } else if field == "chat" {
                from = if ordered {
                    "chats c CROSS JOIN LATERAL (SELECT * FROM chat_messages m
                      WHERE m.chat_id=c.id AND m.deleted_at IS NULL AND m.search_text IS NOT NULL OFFSET 0)m".to_owned()
                } else {
                    "chat_messages m JOIN chats c ON c.id=m.chat_id".to_owned()
                };
                if ordered {
                    // These predicates are inside the indexed entrance, before its metadata is returned.
                    live = "true".to_owned();
                }
            }
            if scope.is_some_and(|scope| !scope.documents.is_empty()) {
                // Separate indexed entrances avoid an OR choosing a global recency scan.
                from = from.replace(
                    "documents d",
                    "(SELECT * FROM documents WHERE node_id=ANY($21::uuid[])
                  UNION ALL SELECT * FROM documents WHERE id=ANY($22::uuid[])
                    AND NOT(node_id=ANY($21::uuid[]))) d",
                );
            }
        }
        let entrance = if filtered_source && !(field == "revision" && recent_sample) {
            format!("FROM {from} JOIN nodes n ON n.id={}", self.node)
        } else {
            format!(
                "FROM {from} CROSS JOIN LATERAL (SELECT * FROM nodes n WHERE n.id={} OFFSET 0)n",
                self.node
            )
        };
        let mut sql = sql.replace(
            "FROM search_fields s JOIN nodes n ON n.id=s.node_id",
            &entrance,
        );
        sql = sql.replace(
            "WHERE __CANDIDATE_ACCESS__",
            &format!("WHERE ({live}) AND __CANDIDATE_ACCESS__"),
        );
        let scoped_access = scope.map(|scope| {
            let kind = match self.kind {
                "'document'::text" => "document",
                "'chat_message'::text" => "chat_message",
                "'item'::text" => "item",
                _ => "name",
            };
            // Source liveness remains above, and the final payload still passes its current frame.
            format!(
                "({}) AND $20::jsonb IS NOT NULL",
                scope.predicate(kind, "s.node_id", "s.source_id", 20)
            )
        });
        sql = sql.replace(
            "__CANDIDATE_ACCESS__",
            scoped_access.as_deref().unwrap_or(access),
        );
        let field = format!("'{field}'::text");
        for (alias, expr) in [
            ("s.source_id", self.source),
            ("s.part_id", self.part),
            ("s.node_id", self.node),
            ("s.block_id", self.block),
            ("s.author_id", self.author),
            ("s.item_kind", self.item),
            ("s.has_file", self.file),
            ("s.mentions", self.mentions),
            ("s.kind", self.kind),
            ("s.field", field.as_str()),
            ("s.tier", self.tier),
            ("s.text", self.text),
            ("s.at", self.at),
        ] {
            sql = sql.replace(alias, expr);
        }
        sql
    }

    /// Read the text only after a recent window's identities have been bounded.
    pub(super) fn deferred_text(&self, field: &str) -> Option<&'static str> {
        match field {
            "name" => Some(
                "SELECT current.name AS text FROM nodes current WHERE current.id=s.part_id OFFSET 0",
            ),
            "node_description" => Some(
                "SELECT current.description AS text FROM nodes current WHERE current.id=s.part_id OFFSET 0",
            ),
            "title" => Some(
                "SELECT current.title AS text FROM documents current WHERE current.id=s.part_id OFFSET 0",
            ),
            "description" => Some(
                "SELECT current.description AS text FROM documents current WHERE current.id=s.part_id OFFSET 0",
            ),
            "heading" | "block" => Some(
                "SELECT current.text AS text FROM document_blocks current WHERE current.id=s.part_id OFFSET 0",
            ),
            "chat" => Some(
                "SELECT current.search_text AS text FROM chat_messages current WHERE current.id=s.part_id OFFSET 0",
            ),
            "item" => Some(
                "SELECT current.search_text AS text FROM items current WHERE current.id=s.part_id OFFSET 0",
            ),
            _ => None,
        }
    }
}
