//! Worktree-scoped localhost labels backed by a loopback TCP proxy.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const HEADER_LIMIT: usize = 64 * 1024;
const LABEL_MAX: usize = 48;

fn registry() -> &'static Mutex<HashMap<String, u16>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, u16>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn slugify_label(value: &str) -> String {
    let lowered: String = value.trim().chars().flat_map(char::to_lowercase).collect();
    let mut slug = String::new();
    let mut separator = false;
    for character in lowered.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            if slug.len() < LABEL_MAX {
                slug.push(character);
            }
        } else if character != '\'' && character != '"' {
            separator = true;
        }
        if slug.len() >= LABEL_MAX {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "workspace".to_string()
    } else {
        slug
    }
}

pub fn worktree_host_label(project_name: &str, worktree_name: &str) -> String {
    let short = worktree_name
        .split(['/', '\\'])
        .rev()
        .find(|part| !part.trim().is_empty())
        .unwrap_or(worktree_name);
    let worktree = slugify_label(short);
    if worktree == "main" || worktree_name.to_ascii_lowercase().ends_with("/main") {
        slugify_label(&format!("{}-main", slugify_label(project_name)))
    } else {
        worktree
    }
}

pub fn listener_url(address: &str) -> Option<String> {
    let port = address
        .rsplit_once(':')?
        .1
        .trim_end_matches(']')
        .parse::<u16>()
        .ok()?;
    Some(format!("http://127.0.0.1:{port}/"))
}

fn rewrite_host(header: &[u8], authority: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(header);
    let mut output = String::new();
    for line in text.split_inclusive("\r\n") {
        if line
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("host:"))
        {
            output.push_str("Host: ");
            output.push_str(authority);
            output.push_str("\r\n");
        } else {
            output.push_str(line);
        }
    }
    output.into_bytes()
}

async fn proxy_connection(mut incoming: TcpStream, target: String) -> Result<(), String> {
    let mut outgoing = TcpStream::connect(&target)
        .await
        .map_err(|error| format!("Could not connect to {target}: {error}"))?;
    let mut header = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let read = incoming
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(());
        }
        header.extend_from_slice(&chunk[..read]);
        if header.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if header.len() > HEADER_LIMIT {
            return Err("Localhost proxy request headers were too large.".to_string());
        }
    }
    outgoing
        .write_all(&rewrite_host(&header, &target))
        .await
        .map_err(|error| error.to_string())?;
    tokio::io::copy_bidirectional(&mut incoming, &mut outgoing)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn register(
    target_url: String,
    project_name: String,
    worktree_name: String,
) -> Result<String, String> {
    let parsed = url::Url::parse(&target_url).map_err(|error| error.to_string())?;
    if parsed.scheme() != "http" {
        return Err("Only HTTP localhost ports can be labeled.".to_string());
    }
    let target_port = parsed
        .port()
        .ok_or_else(|| "The localhost URL has no explicit port.".to_string())?;
    let target = format!("127.0.0.1:{target_port}");
    let base = worktree_host_label(&project_name, &worktree_name);
    let route_key = format!("{base}\0{target_url}");
    if let Some(port) = registry().lock().unwrap().get(&route_key).copied() {
        return Ok(format!("http://{base}.orca.localhost:{port}/"));
    }

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| format!("Could not start localhost label proxy: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    registry().lock().unwrap().insert(route_key, port);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let target = target.clone();
            tokio::spawn(async move {
                let _ = proxy_connection(stream, target).await;
            });
        }
    });
    Ok(format!("http://{base}.orca.localhost:{port}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_orca_normalization_and_main_scoping() {
        assert_eq!(slugify_label("  Fix/Auth \"Flow\"  "), "fix-auth-flow");
        assert_eq!(slugify_label("🔥"), "workspace");
        assert_eq!(
            worktree_host_label("Acme App", "/tmp/main"),
            "acme-app-main"
        );
        assert_eq!(worktree_host_label("Acme", "/tmp/Fix Auth"), "fix-auth");
    }

    #[test]
    fn listener_addresses_become_connectable_http_urls() {
        assert_eq!(
            listener_url("127.0.0.1:5173").as_deref(),
            Some("http://127.0.0.1:5173/")
        );
        assert_eq!(
            listener_url("*:8080").as_deref(),
            Some("http://127.0.0.1:8080/")
        );
        assert_eq!(listener_url("not-a-listener"), None);
    }

    #[tokio::test]
    async fn proxy_rewrites_host_and_streams_the_response() {
        let target = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = target.accept().await.unwrap();
            let mut request = vec![0u8; 4096];
            let size = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..size]);
            assert!(request.contains(&format!("Host: 127.0.0.1:{target_port}")));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                .await
                .unwrap();
        });

        let labeled = register(
            format!("http://127.0.0.1:{target_port}/"),
            "Acme".into(),
            "Nautilus".into(),
        )
        .await
        .unwrap();
        let proxy_port = url::Url::parse(&labeled).unwrap().port().unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: nautilus.orca.localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.ends_with(b"\r\n\r\nOK"));
        server.await.unwrap();
    }
}
