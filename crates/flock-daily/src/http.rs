//! The blocking HTTPS source: two Esplora-style explorers, run on a background
//! thread. TLS is rustls with the bundled webpki roots (ureq 2.x, `tls`
//! feature); there is no native-tls and no openssl.

use crate::resolve::{resolve, Outcome, Source};
use crate::{hash_from_display, hash_to_display, Error, Header};
use std::io::Read;
use std::time::Duration;

/// Per-request timeout.
pub const TIMEOUT: Duration = Duration::from_secs(8);
/// The User-Agent: plain, no contact details.
pub const USER_AGENT: &str = "clusterflock";
/// Largest body read from an explorer. Every answer is below 400 bytes.
const MAX_BODY: u64 = 4096;

pub const MEMPOOL: &str = "https://mempool.space/api";
pub const BLOCKSTREAM: &str = "https://blockstream.info/api";

/// One explorer.
pub struct Explorer {
    name: &'static str,
    base: String,
    agent: ureq::Agent,
}

impl Explorer {
    pub fn new(name: &'static str, base: &str) -> Explorer {
        let agent = ureq::AgentBuilder::new()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .redirects(0)
            .build();
        Explorer { name, base: base.trim_end_matches('/').to_string(), agent }
    }

    fn get(&self, path: &str) -> Result<String, Error> {
        let url = format!("{}{}", self.base, path);
        let net = |what: String| Error::Network { source: self.name.to_string(), what };
        let resp = self.agent.get(&url).call().map_err(|e| match e {
            ureq::Error::Status(code, _) => net(format!("HTTP {code}")),
            ureq::Error::Transport(t) => net(t.to_string()),
        })?;
        let mut body = String::new();
        resp.into_reader().take(MAX_BODY).read_to_string(&mut body).map_err(|e| net(e.to_string()))?;
        Ok(body.trim().to_string())
    }

    fn bad(&self, what: &str) -> Error {
        Error::BadResponse { source: self.name.to_string(), what: what.to_string() }
    }
}

impl Source for Explorer {
    fn name(&self) -> &str {
        self.name
    }

    fn tip_height(&mut self) -> Result<u32, Error> {
        self.get("/blocks/tip/height")?.parse().map_err(|_| self.bad("tip height"))
    }

    fn hash_at(&mut self, height: u32) -> Result<[u8; 32], Error> {
        let s = self.get(&format!("/block-height/{height}"))?;
        hash_from_display(&s).map_err(|_| self.bad("block hash"))
    }

    fn header(&mut self, hash: &[u8; 32]) -> Result<Header, Error> {
        let s = self.get(&format!("/block/{}/header", hash_to_display(hash)))?;
        Header::from_hex(&s).map_err(|_| self.bad("header"))
    }

    fn height_hint(&mut self, ts: u64) -> Result<u32, Error> {
        let s = self.get(&format!("/v1/mining/blocks/timestamp/{ts}"))?;
        let v: serde_json::Value = serde_json::from_str(&s).map_err(|_| self.bad("timestamp lookup"))?;
        v.get("height")
            .and_then(|h| h.as_u64())
            .and_then(|h| u32::try_from(h).ok())
            .ok_or_else(|| self.bad("timestamp lookup"))
    }
}

/// Resolve today's daily over the network: mempool.space, cross-checked on
/// blockstream.info. Blocks; call from a background thread.
pub fn fetch_today() -> Result<Outcome, Error> {
    let mut primary = Explorer::new("mempool.space", MEMPOOL);
    let mut second = Explorer::new("blockstream.info", BLOCKSTREAM);
    resolve(&mut primary, Some(&mut second))
}
