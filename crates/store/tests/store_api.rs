//! End-to-end checks through the public API only: a file-backed database, concurrent
//! handles, branching, search, audit export and secrets.

use std::collections::HashSet;

use serde_json::json;
use store::{
    AuditDecision, AuditEntry, AuditQuery, ChunkedSecretStore, ListChats, MemorySecretStore,
    MessageStatus, NewChat, NewMessage, Role, SecretStore, Store,
};

fn user_message(chat: store::ChatId, parent: Option<store::MessageId>, text: &str) -> NewMessage {
    NewMessage {
        chat_id: chat,
        parent_id: parent,
        role: Role::User,
        content: json!([{ "type": "text", "text": text }]),
        reasoning: None,
        status: MessageStatus::Complete,
        model_ref: None,
        search_text: text.to_owned(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hundred_concurrent_inserts_through_cloned_handles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("app.sqlite3"))
        .await
        .expect("open");
    let chat = store
        .create_chat(NewChat {
            title: "busy".into(),
            ..NewChat::default()
        })
        .await
        .expect("chat");

    let mut tasks = Vec::new();
    for i in 0..100 {
        let handle = store.clone();
        tasks.push(tokio::spawn(async move {
            handle
                .insert_message(user_message(chat, None, &format!("message {i}")))
                .await
        }));
    }
    let mut ids = HashSet::new();
    for task in tasks {
        let id = task.await.expect("task joined").expect("insert succeeded");
        assert!(ids.insert(id), "ids are unique");
    }
    assert_eq!(ids.len(), 100);

    let leaf = store
        .get_chat(chat)
        .await
        .expect("get")
        .expect("chat exists")
        .leaf_message_id
        .expect("leaf set");
    assert!(ids.contains(&leaf));
    // Every message is a root, so they are all siblings of each other.
    assert_eq!(store.siblings(leaf).await.expect("siblings").len(), 100);
}

#[tokio::test]
async fn file_backed_flow_survives_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("data").join("app.sqlite3");
    let (chat, first_answer, second_answer) = {
        let store = Store::open(&path).await.expect("open");
        let chat = store
            .create_chat(NewChat {
                title: "Tokyo trip".into(),
                ..NewChat::default()
            })
            .await
            .expect("chat");
        let question = store
            .insert_message(user_message(chat, None, "東京タワーの高さは？"))
            .await
            .expect("question");
        let mut answer = user_message(chat, Some(question), "333 m");
        answer.role = Role::Assistant;
        let first = store.insert_message(answer.clone()).await.expect("answer");
        answer.search_text = "about 333 metres".into();
        let second = store.insert_message(answer).await.expect("regenerated");
        store
            .append_audit(AuditEntry {
                ts: None,
                chat_id: Some(chat),
                server: "files".into(),
                tool: "read_file".into(),
                arguments: json!({ "path": "C:\\Users\\me\\notes, draft.txt" }),
                preview: "Read C:\\Users\\me\\notes, draft.txt".into(),
                risk: "read".into(),
                decision: AuditDecision::AllowOnce,
                decided_by: "user".into(),
                note: None,
                parent_audit_id: None,
                result: None,
            })
            .await
            .expect("audit");
        (chat, first, second)
    };

    let store = Store::open(&path).await.expect("reopen");
    let path_now = store.current_path(chat).await.expect("current");
    assert_eq!(path_now.last().map(|m| m.id), Some(second_answer));
    assert_eq!(
        store.siblings(first_answer).await.expect("siblings"),
        vec![first_answer, second_answer]
    );

    let hits = store
        .list_chats(ListChats {
            query: Some("京タワ".into()),
            ..ListChats::default()
        })
        .await
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, chat);

    let csv = store
        .export_audit_csv(AuditQuery::default())
        .await
        .expect("csv");
    assert!(
        csv.contains("\"Read C:\\Users\\me\\notes, draft.txt\""),
        "{csv}"
    );

    // Deleting the chat keeps its audit trail.
    store.delete_chat(chat).await.expect("delete");
    assert_eq!(
        store
            .list_audit(AuditQuery::default())
            .await
            .expect("audit")
            .len(),
        1
    );
}

#[test]
fn secrets_through_trait_objects() {
    let store: Box<dyn SecretStore> = Box::new(ChunkedSecretStore::new(
        MemorySecretStore::with_max_value_bytes(2560),
    ));
    let long = "k".repeat(10_240);
    store.set("hf_token", &long).expect("set");
    assert_eq!(
        store.get("hf_token").expect("get").as_deref(),
        Some(long.as_str())
    );
    store.delete("hf_token").expect("delete");
    assert_eq!(store.get("hf_token").expect("get"), None);
}

/// Tauri commands need `Send` futures and shareable handles.
#[test]
fn handles_and_futures_are_send() {
    fn assert_send_sync_clone<T: Send + Sync + Clone + 'static>() {}
    fn assert_send<T: Send>(_: &T) {}
    assert_send_sync_clone::<Store>();
    let store = Store::open_in_memory().expect("open");
    assert_send(&store.list_chats(ListChats::default()));
    assert_send(&store.set_setting("k", &1_u8));
    assert_send(&store.get_setting::<String>("k"));
    assert_send(&store.latest_summary_on_path(store::ChatId(1), &[]));
    fn assert_secret_store<T: SecretStore + 'static>() {}
    assert_secret_store::<Box<dyn SecretStore>>();
    assert_secret_store::<std::sync::Arc<store::KeychainSecretStore>>();
}
