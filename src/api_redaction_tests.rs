//! The embedding endpoint under the redaction policy: a text given to be
//! embedded is prepared before the model is given it, and what the
//! endpoint says of a failure or a bad request is fixed. Credential
//! fixtures are assembled at run time; assertions never print them.
use std::path::Path;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::redaction::CREDENTIAL_MARKER;
use crate::test_support::RecordingEmbedder;

fn token() -> String {
    ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat()
}

/// A server on a socket of its own, with `embedder` as its model.
async fn serve(embedder: RecordingEmbedder) -> crate::test_support::InTempDir<std::path::PathBuf> {
    let socket = crate::test_support::temp_socket_path("mn-embed-");
    let db = socket.parent().unwrap().join("memory.db");
    let storage = Arc::new(Storage::open(&db).unwrap());
    let server = ApiServer::new(socket.to_path_buf(), storage, Arc::new(embedder));
    tokio::spawn(server.start());
    for _ in 0..200 {
        if tokio::net::UnixStream::connect(&*socket).await.is_ok() {
            return socket;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the server did not start");
}

async fn post(socket: &Path, body: &str) -> (u16, String) {
    let mut stream = tokio::net::UnixStream::connect(socket).await.unwrap();
    let request = format!(
        "POST /embed HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap();
    (status, response)
}

#[tokio::test]
async fn redaction_generated_embed_endpoint_gives_the_model_prepared_text() {
    let token = token();
    let embedder = RecordingEmbedder::new();
    let socket = serve(embedder.clone()).await;
    for kind in ["passage", "query"] {
        let body = serde_json::json!({
            "kind": kind,
            "texts": [format!("deploy with {token} on friday"), "the window moved"],
        })
        .to_string();
        let (status, response) = post(&socket, &body).await;
        assert!(status == 200);
        assert!(!response.contains(&token));
    }
    let texts = embedder.texts();
    assert!(texts.len() == 4);
    assert!(texts.iter().all(|text| !text.contains(&token)));
    assert!(texts[0].contains(CREDENTIAL_MARKER) && texts[1] == "the window moved");
}

#[tokio::test]
async fn redaction_generated_embed_endpoint_says_a_fixed_word_of_a_failure() {
    let token = token();
    let socket = serve(RecordingEmbedder::failing()).await;
    let body = serde_json::json!({"kind": "passage", "texts": [format!("the word is {token}")]})
        .to_string();
    let (status, response) = post(&socket, &body).await;
    assert!(status == 500);
    assert!(response.contains("EMBEDDING_FAILED"));
    // Neither the text, nor the words of the embedder that quote it.
    assert!(!response.contains(&token) && !response.contains("failed on"));
}

/// A bad request is said in fixed words: a parser quotes the body it
/// stopped in, and a field holds what the caller put there.
#[test]
fn redaction_generated_embed_request_errors_quote_nothing_of_the_request() {
    let token = token();
    let bad = [
        format!("{{\"kind\": \"query\", \"texts\": [\"a\"], {token}"),
        serde_json::json!({"kind": token, "texts": ["a"]}).to_string(),
        serde_json::json!({"kind": "query", "texts": [{"nested": token}]}).to_string(),
        serde_json::json!({"kind": "query", token.as_str(): ["a"]}).to_string(),
    ];
    let mut all = Vec::new();
    for body in bad {
        let said = parse_embed_request(body.as_bytes()).unwrap_err();
        assert!(!said.contains(&token), "the error quotes the request");
        all.push(said);
    }
    // The words are fixed: a parser's own say where it stopped.
    assert!(all[0] == "bad JSON");
    assert!(all[1] == "bad 'kind' (\"query\" or \"passage\")");
}

async fn get(socket: &Path, path: &str) -> (u16, String) {
    let mut stream = tokio::net::UnixStream::connect(socket).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap();
    (status, response)
}

/// A store that fails a query is said in a fixed word: what the store
/// says of its failure stays in the process.
#[tokio::test]
async fn redaction_outputs_socket_says_a_fixed_word_of_a_store_failure() {
    let socket = serve(RecordingEmbedder::new()).await;
    let db = socket.parent().unwrap().join("memory.db");
    // A store whose tables are gone under the server.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let triggers: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'trigger'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for trigger in triggers {
        conn.execute_batch(&format!("DROP TRIGGER \"{trigger}\""))
            .unwrap();
    }
    conn.execute_batch("DROP TABLE memories_fts; ALTER TABLE memories RENAME TO memories_gone;")
        .unwrap();
    drop(conn);
    for path in ["/query/rollout", "/recent"] {
        let (status, response) = get(&socket, path).await;
        assert!(status == 500);
        assert!(response.contains("STORAGE_FAILED"));
        assert!(!response.contains("no such") && !response.contains("memories"));
    }
}
