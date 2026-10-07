//! Send registered Windows URL launches to the window that owns the profile.
//!
//! Windows starts the protocol command even when F1R3Gaze already runs. The
//! second process cannot take the profile lock, so the window publishes a
//! loopback endpoint and a random token under that profile's runtime root.
//! Only a lock holder recorded as a window may receive the handoff.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const ENDPOINT_FILE: &str = "url-handoff.json";
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const POLL: Duration = Duration::from_millis(20);
const HANDOFF_DEADLINE: Duration = Duration::from_secs(2);
const IO_DEADLINE: Duration = Duration::from_millis(300);

#[derive(Deserialize, Serialize)]
struct Endpoint {
    port: u16,
    token: String,
}

#[derive(Deserialize, Serialize)]
struct Request {
    token: String,
    url: String,
}

pub(crate) struct Registration {
    endpoint: PathBuf,
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port));
        let _ = TcpStream::connect_timeout(&address, IO_DEADLINE);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if fs::read(&self.endpoint)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Endpoint>(&bytes).ok())
            .is_some_and(|endpoint| endpoint.token == self.token)
        {
            let _ = fs::remove_file(&self.endpoint);
        }
    }
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|error| format!("URL handoff token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn handle(mut stream: TcpStream, token: &str, sender: &mpsc::Sender<String>, wake: &dyn Fn()) {
    let _ = stream.set_read_timeout(Some(IO_DEADLINE));
    let _ = stream.set_write_timeout(Some(IO_DEADLINE));
    let mut bytes = Vec::new();
    if Read::by_ref(&mut stream)
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_REQUEST_BYTES
    {
        return;
    }
    let Ok(request) = serde_json::from_slice::<Request>(&bytes) else {
        return;
    };
    if request.token != token {
        return;
    }
    let Some(url) = crate::external_url::accepted_url(&request.url) else {
        return;
    };
    if sender.send(url).is_ok() {
        wake();
        let _ = stream.write_all(b"1");
    }
}

/// Publish a local endpoint for this window and wake its event loop on URLs.
pub(crate) fn start(
    runtime: &Path,
    wake: impl Fn() + Send + 'static,
) -> Result<(Registration, mpsc::Receiver<String>), String> {
    fs::create_dir_all(runtime).map_err(|error| format!("URL handoff directory: {error}"))?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("URL handoff listener: {error}"))?;
    let token = random_token()?;
    let endpoint = runtime.join(ENDPOINT_FILE);
    let temporary = runtime.join(format!(
        "{ENDPOINT_FILE}-{}-{token}.tmp",
        std::process::id()
    ));
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let published = Endpoint {
        port,
        token: token.clone(),
    };
    let bytes = serde_json::to_vec(&published).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| format!("URL handoff endpoint: {error}"))?;
    if endpoint.exists() {
        fs::remove_file(&endpoint)
            .map_err(|error| format!("stale URL handoff endpoint: {error}"))?;
    }
    if let Err(error) = fs::rename(&temporary, &endpoint) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("publish URL handoff endpoint: {error}"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let (sender, receiver) = mpsc::channel();
    let thread_token = token.clone();
    let worker = thread::Builder::new()
        .name("gaze-url-handoff".into())
        .spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                handle(stream, &thread_token, &sender, &wake);
            }
        })
        .map_err(|error| {
            let _ = fs::remove_file(&endpoint);
            format!("URL handoff thread: {error}")
        })?;
    Ok((
        Registration {
            endpoint,
            port,
            token,
            stop,
            worker: Some(worker),
        },
        receiver,
    ))
}

/// A second process calls this only after the profile lock reports a window.
/// The acknowledgment means the running window queued this exact URL.
pub fn forward(runtime: &Path, raw: &str) -> bool {
    let Some(url) = crate::external_url::accepted_url(raw) else {
        return false;
    };
    let endpoint = runtime.join(ENDPOINT_FILE);
    let deadline = Instant::now() + HANDOFF_DEADLINE;
    while Instant::now() < deadline {
        if let Ok(bytes) = fs::read(&endpoint) {
            if let Ok(published) = serde_json::from_slice::<Endpoint>(&bytes) {
                let address =
                    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, published.port));
                if let Ok(mut stream) = TcpStream::connect_timeout(&address, IO_DEADLINE) {
                    let _ = stream.set_read_timeout(Some(IO_DEADLINE));
                    let _ = stream.set_write_timeout(Some(IO_DEADLINE));
                    let request = Request {
                        token: published.token,
                        url: url.clone(),
                    };
                    if let Ok(bytes) = serde_json::to_vec(&request) {
                        if stream.write_all(&bytes).is_ok()
                            && stream.shutdown(Shutdown::Write).is_ok()
                        {
                            let mut acknowledgment = [0u8; 1];
                            if stream.read_exact(&mut acknowledgment).is_ok()
                                && acknowledgment == *b"1"
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        thread::sleep(POLL);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_handoff_delivers_only_valid_addresses() {
        let dir = std::env::temp_dir().join(format!("gaze-url-test-{}", random_token().unwrap()));
        let (wake_sender, wake_receiver) = mpsc::channel();
        let (registration, received) = start(&dir, move || {
            let _ = wake_sender.send(());
        })
        .unwrap();
        assert!(forward(&dir, "F1R3://ABCD/project"));
        assert_eq!(
            received.recv_timeout(HANDOFF_DEADLINE).unwrap(),
            "f1r3://ABCD/project"
        );
        wake_receiver.recv_timeout(HANDOFF_DEADLINE).unwrap();
        let content = format!("f1r3h://blake2b-256/{}", "a".repeat(64));
        assert!(forward(&dir, &content));
        assert_eq!(received.recv_timeout(HANDOFF_DEADLINE).unwrap(), content);
        wake_receiver.recv_timeout(HANDOFF_DEADLINE).unwrap();
        assert!(!forward(&dir, "https://example.org"));
        let endpoint: Endpoint =
            serde_json::from_slice(&fs::read(dir.join(ENDPOINT_FILE)).unwrap()).unwrap();
        let mut impostor = TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port)).unwrap();
        impostor.set_read_timeout(Some(HANDOFF_DEADLINE)).unwrap();
        let forged = Request {
            token: "wrong-token".into(),
            url: "f1r3://abcd/project".into(),
        };
        impostor
            .write_all(&serde_json::to_vec(&forged).unwrap())
            .unwrap();
        impostor.shutdown(Shutdown::Write).unwrap();
        assert_eq!(impostor.read(&mut [0u8; 1]).unwrap(), 0);
        assert!(received.try_recv().is_err());
        drop(registration);
        assert!(!dir.join(ENDPOINT_FILE).exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
