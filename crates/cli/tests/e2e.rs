use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use platform_cli::{execute, serve, Command};
use platform_store::SqliteStore;
use tokio::net::TcpListener;

const NOW: i64 = 1_700_000_000;

fn temp_db() -> PathBuf {
    std::env::temp_dir().join(format!("platform-e2e-{}.sqlite", std::process::id()))
}

fn open(path: &PathBuf) -> SqliteStore {
    match SqliteStore::open(path) {
        Ok(s) => s,
        Err(e) => panic!("cannot open {path:?}: {e}"),
    }
}

fn run(store: &SqliteStore, command: Command) -> String {
    match execute(store, &command, NOW) {
        Ok(out) => out,
        Err(e) => panic!("{command:?} failed: {e}"),
    }
}

fn get(addr: SocketAddr, path: &str, token: &str) -> (u16, String) {
    let Ok(mut stream) = TcpStream::connect(addr) else {
        panic!("cannot connect to {addr}");
    };
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    assert!(stream.write_all(request.as_bytes()).is_ok());
    let mut raw = String::new();
    assert!(stream.read_to_string(&mut raw).is_ok());
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
    (status, body)
}

fn premium(addr: SocketAddr, token: &str) -> String {
    let (code, body) = get(addr, "/v1/premium/10", token);
    assert_eq!(code, 200, "{body}");
    body
}

#[tokio::test(flavor = "multi_thread")]
async fn a_purchase_a_cancel_and_a_dispute_are_seen_by_the_running_api() {
    let path = temp_db();
    let _ = std::fs::remove_file(&path);
    let store = open(&path);
    let token = run(&store, Command::ProductToken { product: 1 });

    let Ok(listener) = TcpListener::bind("127.0.0.1:0").await else {
        panic!("cannot bind");
    };
    let Ok(addr) = listener.local_addr() else {
        panic!("no local address");
    };
    let server = tokio::spawn(serve(listener, open(&path), Arc::new(|| NOW)));

    let pay = Command::Pay {
        guild: 10,
        product: 1,
        days: 30,
    };
    let cancel = Command::Cancel {
        guild: 10,
        product: 1,
    };
    let dispute = Command::Dispute {
        guild: 10,
        product: 1,
    };
    let won = Command::DisputeWon {
        guild: 10,
        product: 1,
    };

    assert!(premium(addr, &token).contains(r#""status":"expired","access":false"#));

    run(&store, pay.clone());
    assert!(premium(addr, &token).contains(r#""guild_id":"10","status":"active","access":true"#));

    run(&store, cancel);
    assert!(premium(addr, &token).contains(r#""status":"expired","access":false"#));

    run(&store, pay.clone());
    assert!(premium(addr, &token).contains(r#""status":"active""#));

    run(&store, dispute);
    assert!(premium(addr, &token).contains(r#""status":"expired""#));
    run(&store, pay);
    assert!(premium(addr, &token).contains(r#""status":"expired""#));

    run(&store, won);
    assert!(premium(addr, &token).contains(r#""status":"active""#));

    assert_eq!(get(addr, "/v1/premium/10", "wrong-token").0, 401);
    assert_eq!(get(addr, "/v1/premium/abc", &token).0, 400);
    let (_, other_guild) = get(addr, "/v1/premium/11", &token);
    assert!(other_guild.contains(r#""status":"expired""#));

    server.abort();
    drop(store);
    let _ = std::fs::remove_file(&path);
}
