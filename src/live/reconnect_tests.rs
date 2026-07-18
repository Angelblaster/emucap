use super::*;
use std::io::BufRead;
use std::net::TcpListener;

#[test]
fn reconnects_front_session_without_recreating_handler_state() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let mut calls = 0u64;
        serve_reconnecting_inner(
            port,
            "test-bridge",
            move |request| {
                calls += 1;
                Response {
                    id: request.id,
                    ok: true,
                    result: Some(serde_json::json!({"calls": calls})),
                    error: None,
                }
            },
            Some(2),
        )
    });

    for expected in [1, 2] {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .write_all(
                format!("{{\"v\":1,\"id\":{expected},\"method\":\"status\",\"params\":{{}}}}\n")
                    .as_bytes(),
            )
            .unwrap();
        let mut response = String::new();
        BufReader::new(socket.try_clone().unwrap())
            .read_line(&mut response)
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["result"]["calls"], expected);
        drop(socket);
    }
    worker.join().unwrap().unwrap();
}
