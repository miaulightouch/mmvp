//! HTTP: one shared connection pool (thousands of small clip downloads reuse kept-alive connections),
//! retries with backoff for transient errors.
use anyhow::{anyhow, bail, Result};
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

pub const PARALLEL: usize = 16;

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .max_idle_connections_per_host(PARALLEL * 2)
            .build()
    })
}

/// GET `url` into memory; `progress(received, total)` is called while reading (total 0 if unknown).
pub fn get(url: &str, progress: &dyn Fn(u64, u64)) -> Result<Vec<u8>> {
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500 << attempt));
        }
        let resp = match agent().get(url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) if (400..500).contains(&code) => bail!("{url}: HTTP {code}"),
            Err(e) => {
                last = e.to_string();
                continue;
            }
        };
        let total: u64 = resp.header("Content-Length").and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut body = Vec::with_capacity(total as usize);
        let mut r = resp.into_reader().take(512 << 20);
        let mut buf = [0u8; 64 << 10];
        loop {
            match r.read(&mut buf) {
                Ok(0) => return Ok(body),
                Ok(n) => {
                    body.extend_from_slice(&buf[..n]);
                    progress(body.len() as u64, total);
                }
                Err(e) => {
                    last = e.to_string();
                    break;
                }
            }
        }
    }
    Err(anyhow!("{url}: {last}"))
}
