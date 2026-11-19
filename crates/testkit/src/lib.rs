#![forbid(unsafe_code)]
// Faux serveur HTTP local pour tester les clients Discord et GitHub sans réseau.
// Réservé aux tests : jamais une dépendance d'exécution.
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![(
                "Content-Type".to_string(),
                "application/octet-stream".to_string(),
            )],
            body,
        }
    }

    pub fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            headers: vec![("Location".to_string(), location.to_string())],
            body: Vec::new(),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buffer = [0u8; 1024];
    while let Ok(n) = stream.read(&mut buffer) {
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..n]);
        if let Some(end) = find(&data, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

fn write_reply(stream: &mut TcpStream, reply: &Reply) {
    let mut head = format!("HTTP/1.1 {} X\r\n", reply.status);
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.body.len()
    ));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&reply.body);
}

fn accept(listener: &TcpListener, deadline: Instant) -> Option<TcpStream> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                return Some(stream);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

fn bind() -> (TcpListener, String) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        panic!("cannot bind");
    };
    let Ok(addr) = listener.local_addr() else {
        panic!("no local address");
    };
    let _ = listener.set_nonblocking(true);
    (listener, format!("http://{addr}"))
}

// Une réponse par connexion, dans l'ordre. Borné dans le temps : si le client fait moins de
// requêtes que prévu, le test échoue au lieu de bloquer.
pub fn fake_server(replies: Vec<Reply>) -> (String, JoinHandle<Vec<String>>) {
    let (listener, base) = bind();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let deadline = Instant::now() + DEADLINE;
        for reply in replies {
            let Some(mut stream) = accept(&listener, deadline) else {
                return seen;
            };
            seen.push(read_request(&mut stream));
            write_reply(&mut stream, &reply);
        }
        seen
    });
    (base, handle)
}

pub fn fake_discord(responses: Vec<(u16, &'static str)>) -> (String, JoinHandle<Vec<String>>) {
    fake_server(
        responses
            .into_iter()
            .map(|(status, body)| Reply::json(status, body))
            .collect(),
    )
}

// Répond selon « MÉTHODE chemin » (début de la ligne de requête), au plus `max` requêtes ;
// 404 pour ce qui ne correspond à aucune route. Renvoie les requêtes vues.
pub fn fake_routes(routes: Vec<(String, Reply)>, max: usize) -> (String, JoinHandle<Vec<String>>) {
    let (listener, base) = bind();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let deadline = Instant::now() + DEADLINE;
        for _ in 0..max {
            let Some(mut stream) = accept(&listener, deadline) else {
                break;
            };
            let request = read_request(&mut stream);
            let line = request.lines().next().unwrap_or_default().to_string();
            let reply = routes
                .iter()
                .find(|(prefix, _)| line.starts_with(prefix.as_str()))
                .map_or_else(|| Reply::json(404, "{}"), |(_, r)| r.clone());
            seen.push(request);
            write_reply(&mut stream, &reply);
        }
        seen
    });
    (base, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(base: &str, path: &str) -> (u16, String) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let Ok(mut response) = agent.get(format!("{base}{path}")).call() else {
            panic!("request failed");
        };
        let status = response.status().as_u16();
        (
            status,
            response.body_mut().read_to_string().unwrap_or_default(),
        )
    }

    #[test]
    fn replies_are_served_in_order_and_requests_recorded() {
        let (base, handle) = fake_discord(vec![(200, "one"), (418, "two")]);
        assert_eq!(get(&base, "/a"), (200, "one".to_string()));
        assert_eq!(get(&base, "/b"), (418, "two".to_string()));
        let seen = handle.join().unwrap_or_default();
        assert!(seen[0].starts_with("GET /a ") && seen[1].starts_with("GET /b "));
    }

    #[test]
    fn routes_match_by_method_and_path_prefix_with_a_404_otherwise() {
        let routes = vec![
            ("GET /x".to_string(), Reply::json(200, "x")),
            ("POST /x".to_string(), Reply::json(201, "posted")),
        ];
        let (base, handle) = fake_routes(routes, 2);
        assert_eq!(get(&base, "/x/deeper"), (200, "x".to_string()));
        assert_eq!(get(&base, "/nope").0, 404);
        assert_eq!(handle.join().map(|s| s.len()).ok(), Some(2));
    }

    #[test]
    fn a_redirect_is_followed_to_the_second_route() {
        let routes = vec![
            ("GET /from".to_string(), Reply::redirect("/to")),
            ("GET /to".to_string(), Reply::bytes(200, b"data".to_vec())),
        ];
        let (base, _handle) = fake_routes(routes, 2);
        assert_eq!(get(&base, "/from"), (200, "data".to_string()));
    }
}
