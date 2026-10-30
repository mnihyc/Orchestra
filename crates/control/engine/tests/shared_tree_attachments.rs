//! Attachment directory discovery and path identity, independently of raw-byte download delivery.

mod common;
mod shared;

use orch_api_contract::chats::{ChatKind, ChatPart, ChatRef};
use orch_api_contract::threads::{
    Forwarded, ForwardedAttachment, ForwardedFrom, ForwardedImage, ForwardedType, ImageMediaType,
};
use orch_db::queries::access;
use orch_link_contract::commands::OrchTool;
use orch_types::{Hash, Json, Time, new_id};
use serde_json::json;
use shared::{call, entry, ok, refused, share};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn text<'a>(value: &'a Json, key: &str) -> TestResult<&'a str> {
    value
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("missing {key} in {value}").into())
}

#[tokio::test]
async fn both_chat_file_directories_are_discoverable_immutable_and_match_the_written_carried_order()
-> TestResult {
    let world = shared::a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Attachment reader").await;
    let topic = world.topic("File review").await;
    let node_chat = shared::chat_of(&world, topic).await;
    let people_chat = new_id();
    let message = new_id();
    let mut conn = world.pool.acquire().await?;
    sqlx::query("INSERT INTO chats (id,kind,org_id,name,created_by) VALUES ($1,'group',$2,'Review files',$3)")
        .bind(people_chat).bind(world.root).bind(world.ada).execute(&mut *conn).await?;
    sqlx::query(
        "INSERT INTO chat_members (chat_id,user_id,added_by,seq_updated) VALUES ($1,$2,$2,1)",
    )
    .bind(people_chat)
    .bind(world.ada)
    .execute(&mut *conn)
    .await?;
    let hash = Hash::of(b"synthetic metadata-only attachment");
    let parts = vec![
        ChatPart::Text {
            text: "One direct file, an image and a retained forwarded pair.".into(),
        },
        ChatPart::Attachment {
            blob: hash,
            name: "report.bin".into(),
            media_type: "application/octet-stream".into(),
            size: 12,
        },
        ChatPart::Image {
            blob: hash,
            name: "a/screen.png".into(),
            media_type: ImageMediaType::Png,
        },
        ChatPart::Forwarded(Box::new(Forwarded {
            r#type: ForwardedType::Forwarded,
            from: ForwardedFrom::Chat {
                chat: ChatRef {
                    id: new_id(),
                    kind: ChatKind::Group,
                    org_id: world.root,
                    node: None,
                    name: None,
                },
            },
            source_id: new_id(),
            author: None,
            at: Time::new(world.clock.now()),
            text: "Retained material.".into(),
            images: vec![ForwardedImage {
                blob: hash,
                name: "a/screen.png".into(),
                media_type: "image/png".into(),
            }],
            attachments: vec![ForwardedAttachment {
                blob: hash,
                name: "report.bin".into(),
                media_type: "application/octet-stream".into(),
                size: 23,
            }],
        })),
    ];
    for chat in [node_chat, people_chat] {
        sqlx::query("INSERT INTO chat_messages (id,chat_id,author_id,content,seq_created,seq_updated,deleted_at) VALUES ($1,$2,$3,'[]',40,40,$4)")
            .bind(new_id()).bind(chat).bind(world.ada).bind(world.clock.now()).execute(&mut *conn).await?;
        sqlx::query("INSERT INTO chat_messages (id,chat_id,author_id,content,seq_created,seq_updated) VALUES ($1,$2,$3,$4,90,90)")
            .bind(if chat==node_chat {message} else {new_id()}).bind(chat).bind(world.ada)
            .bind(serde_json::to_value(&parts)?).execute(&mut *conn).await?;
        sqlx::query("UPDATE chats SET next_seq=91,last_message_seq=90 WHERE id=$1")
            .bind(chat)
            .execute(&mut *conn)
            .await?;
    }
    drop(conn);
    let mut read_grants = Vec::new();
    for (kind, object) in [("node", topic), ("chat", people_chat)] {
        read_grants.push(share(&world, kind, object, "live", "chat.view", "any", agent).await);
    }
    // An indexed window still exposes file directories whose chat text was not a candidate.
    {
        let mut conn = world.pool.acquire().await?;
        let asking = orch_authz::reach::Agent::of(&mut conn, agent)
            .await?
            .ok_or("the reading agent is present")?;
        let selected = orch_db::queries::search::CandidateSet::default();
        let indexed = orch_engine::tools::tree::Tree::load_search(
            &mut conn,
            asking,
            orch_authz::reach::AgentReach::None,
            orch_authz::reach::DocumentPolicy::None,
            Time::new(world.clock.now()),
            orch_engine::tools::tree::Scope::Everything,
            &selected,
        )
        .await?;
        for chat in [node_chat, people_chat] {
            assert!(
                indexed.directory_at(orch_engine::tools::tree::Where::ChatFiles(chat)),
                "indexed text windows retain each readable carried-file directory"
            );
        }
    }
    let mut known_paths = Vec::new();
    for chat in [node_chat, people_chat] {
        let read = ok(call(
            &world,
            agent,
            OrchTool::ReadShared,
            json!({"path":format!("id:{chat}")}),
        )
        .await);
        let path = text(&read["entry"], "path")?;
        let parent = path.rsplit_once('/').ok_or("chat parent absent")?.0;
        let parent_listing = ok(call(
            &world,
            agent,
            OrchTool::ListShared,
            json!({"path":format!("{parent}/")}),
        )
        .await);
        let directory = entry(&parent_listing, "chat.files");
        assert_eq!(directory["readable"], true);
        assert_ne!(directory["writable"], true);
        assert_ne!(directory["manageable"], true);
        let directory_path = text(directory, "path")?;
        let listed = ok(call(
            &world,
            agent,
            OrchTool::ListShared,
            json!({"path":directory_path,"sort":"name"}),
        )
        .await);
        let files = listed["entries"].as_array().ok_or("files absent")?;
        let expected = [
            "0002-01-report.bin",
            "0002-02-a_screen.png",
            "0002-03-a_screen.png",
            "0002-04-report.bin",
        ];
        assert_eq!(
            files
                .iter()
                .map(|file| text(file, "name"))
                .collect::<TestResult<Vec<_>>>()?,
            expected
        );
        assert_eq!(files[0]["size"], 12);
        assert_eq!(files[3]["size"], 23);
        let written = text(&read, "text")?;
        for file in files {
            let path = text(file, "path")?;
            let advertised = format!("chat.files/{}", text(file, "name")?);
            assert_eq!(file["kind"], "file");
            assert!(
                written.contains(&advertised),
                "the written link uses the same carried filename: {advertised}"
            );
            assert_eq!(
                path,
                format!("{parent}/{advertised}"),
                "the relative link resolves beside that chat's canonical path"
            );
            let resolved = call(&world, agent, OrchTool::ReadShared, json!({"path":path})).await;
            assert_eq!(refused(&resolved).0, "wrong_kind");
            assert!(refused(&resolved).1.contains("download_shared"));
            known_paths.push(path.to_owned());
        }
        let refusal = call(
            &world,
            agent,
            OrchTool::ManageShared,
            json!({"command":"delete","paths":[directory_path]}),
        )
        .await;
        assert_eq!(refused(&refusal).0, "wrong_kind");
    }
    // Each carried form alone advertises the same directory; empty forwards and deleted messages do not.
    let chat_path = text(
        &ok(call(
            &world,
            agent,
            OrchTool::ReadShared,
            json!({"path":format!("id:{node_chat}")}),
        )
        .await)["entry"],
        "path",
    )?
    .to_owned();
    let parent = chat_path.rsplit_once('/').ok_or("chat parent absent")?.0;
    for (content, deleted, visible) in [
        (serde_json::to_value(&parts[1..2])?, false, true),
        (serde_json::to_value(&parts[2..3])?, false, true),
        (serde_json::to_value(&parts[3..4])?, false, true),
        (
            json!([{"type":"forwarded","images":[],"attachments":[]}]),
            false,
            false,
        ),
        (serde_json::to_value(&parts[..1])?, false, false),
        (serde_json::to_value(&parts[1..2])?, true, false),
    ] {
        sqlx::query("UPDATE chat_messages SET content=$2,deleted_at=$3 WHERE id=$1")
            .bind(message)
            .bind(content)
            .bind(deleted.then(|| world.clock.now()))
            .execute(world.pool.pool())
            .await?;
        let listing = ok(call(
            &world,
            agent,
            OrchTool::ListShared,
            json!({"path":format!("{parent}/")}),
        )
        .await);
        assert_eq!(
            listing["entries"]
                .as_array()
                .ok_or("entries absent")?
                .iter()
                .any(|entry| entry["name"] == "chat.files"),
            visible,
            "directory discovery follows the current live carried material"
        );
    }
    // A revoked owner hides both discovery and previously advertised attachment paths.
    let mut conn = world.pool.acquire().await?;
    for grant in read_grants {
        assert!(access::remove_entry(&mut conn, grant, world.ada, world.clock.now()).await?);
    }
    access::bump_epoch(&mut conn).await?;
    drop(conn);
    for path in known_paths {
        let hidden = call(&world, agent, OrchTool::ReadShared, json!({"path":path})).await;
        assert_eq!(refused(&hidden).0, "not_found");
    }
    for chat in [node_chat, people_chat] {
        let hidden = call(
            &world,
            agent,
            OrchTool::ReadShared,
            json!({"path":format!("id:{chat}")}),
        )
        .await;
        assert_eq!(refused(&hidden).0, "not_found");
        assert!(!serde_json::to_string(&hidden)?.contains("report.bin"));
    }
    Ok(())
}

#[tokio::test]
async fn a_chat_containing_only_pasted_text_keeps_its_files_directory_in_indexed_search()
-> TestResult {
    let world = shared::a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Paste reader").await;
    let topic = world.topic("Pasted notes").await;
    let chat = shared::chat_of(&world, topic).await;
    let parts = vec![ChatPart::PastedText(
        orch_api_contract::threads::PastedText {
            r#type: orch_api_contract::threads::PastedTextType::PastedText,
            blob: Hash::of(b"only pasted words"),
            name: "notes.txt".into(),
            path: ".orch/attachments/notes.txt".into(),
            size: 17,
            characters: 17,
            ascii_characters: 17,
            inline_text: None,
            delivery: None,
            source: None,
        },
    )];
    let mut conn = world.pool.acquire().await?;
    sqlx::query("INSERT INTO chat_messages (id,chat_id,author_id,content,seq_created,seq_updated) VALUES ($1,$2,$3,$4,1,1)")
        .bind(new_id()).bind(chat).bind(world.ada).bind(serde_json::to_value(parts)?)
        .execute(&mut *conn).await?;
    drop(conn);
    share(&world, "node", topic, "live", "chat.view", "any", agent).await;
    let mut conn = world.pool.acquire().await?;
    let asking = orch_authz::reach::Agent::of(&mut conn, agent)
        .await?
        .ok_or("the reading agent is present")?;
    let selected = orch_db::queries::search::CandidateSet::default();
    let indexed = orch_engine::tools::tree::Tree::load_search(
        &mut conn,
        asking,
        orch_authz::reach::AgentReach::None,
        orch_authz::reach::DocumentPolicy::None,
        Time::new(world.clock.now()),
        orch_engine::tools::tree::Scope::Everything,
        &selected,
    )
    .await?;
    assert!(indexed.directory_at(orch_engine::tools::tree::Where::ChatFiles(chat)));
    drop(conn);
    let read = ok(call(
        &world,
        agent,
        OrchTool::ReadShared,
        json!({"path":format!("id:{chat}")}),
    )
    .await);
    let path = text(&read["entry"], "path")?;
    let parent = path.rsplit_once('/').ok_or("chat parent absent")?.0;
    let parent_listing = ok(call(
        &world,
        agent,
        OrchTool::ListShared,
        json!({"path":format!("{parent}/")}),
    )
    .await);
    let directory = entry(&parent_listing, "chat.files");
    let listed = ok(call(
        &world,
        agent,
        OrchTool::ListShared,
        json!({"path":text(directory, "path")?}),
    )
    .await);
    let files = listed["entries"].as_array().ok_or("pasted files absent")?;
    assert_eq!(files.len(), 1);
    assert!(text(&files[0], "name")?.ends_with("notes.txt"));
    assert_eq!(files[0]["description"], "text/plain");
    assert_eq!(files[0]["size"], 17);
    Ok(())
}
