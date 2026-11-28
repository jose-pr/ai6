//! The per-user folder: the identity, the keys of the peers that may connect, the keys of the hosts this user dials, the grants and the
//! names of the hosts.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cabuya::auth::{Identity, Method};
use cabuya::peers::FilePeers;
use coqui_peer::{PeerId, TrustStore};
use serde_json::{Value, json};

/// The folder's files, by name. Nothing here is read until a command asks for it.
pub struct Home {
    dir: PathBuf,
}

/// A host this user paired with: the peer to prove and where its `serve` listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub peer: PeerId,
    pub address: String,
}

impl Home {
    /// `--home`, else `CABUYA_HOME`, else the user's configuration folder.
    pub fn locate(given: Option<PathBuf>) -> Result<Self, String> {
        let dir = match given {
            Some(dir) => dir,
            None => from_environment()?,
        };
        Ok(Self { dir })
    }

    /// The identity, created with `method` (or the default one) when there is none. A key of another method than the one named is
    /// refused.
    pub fn identity(&self, method: Option<Method>) -> Result<Identity, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("cannot create {}: {error}", self.dir.display()))?;
        let path = self.dir.join("identity");
        match method {
            Some(method) => Identity::open_or_create(&path, method),
            None => match Identity::read(&path) {
                Ok(Some(identity)) => Ok(identity),
                Ok(None) => Identity::open_or_create(&path, Method::DEFAULT),
                Err(error) => Err(error),
            },
        }
        .map_err(|error| error.to_string())
    }

    /// The keys of the peers allowed to prove themselves to `serve`, filled by `pair`.
    pub fn served(&self) -> Result<TrustStore, String> {
        self.store("served")
    }

    /// The keys of the hosts this user dials, filled by `pair --join`.
    pub fn dialed(&self) -> Result<TrustStore, String> {
        self.store("dialed")
    }

    fn store(&self, name: &str) -> Result<TrustStore, String> {
        TrustStore::open(&self.dir.join(name))
            .map_err(|error| format!("the {name} keys could not be opened: {error}"))
    }

    /// What each peer may open, in the file `serve` rechecks every second.
    pub fn grants(&self, file: Option<PathBuf>) -> FilePeers {
        FilePeers::new(file.unwrap_or_else(|| self.dir.join("grants.json")))
    }

    fn hosts_file(&self) -> PathBuf {
        self.dir.join("hosts.json")
    }

    pub fn hosts(&self) -> Result<BTreeMap<String, Host>, String> {
        let text = match std::fs::read_to_string(self.hosts_file()) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => return Err(format!("the hosts file could not be read: {error}")),
        };
        let value: Value =
            serde_json::from_str(&text).map_err(|_| "the hosts file is not JSON".to_owned())?;
        let entries = value
            .get("hosts")
            .and_then(Value::as_object)
            .ok_or("the hosts file has no hosts")?;
        entries
            .iter()
            .map(|(name, entry)| {
                let field = |key: &str| entry.get(key).and_then(Value::as_str);
                let peer = field("peer")
                    .and_then(|text| text.parse().ok())
                    .ok_or_else(|| format!("host {name} has no peer id"))?;
                let address = field("address")
                    .ok_or_else(|| format!("host {name} has no address"))?
                    .to_owned();
                Ok((name.clone(), Host { peer, address }))
            })
            .collect()
    }

    /// Records (or replaces) the host called `name`.
    pub fn put_host(&self, name: &str, host: &Host) -> Result<(), String> {
        let mut hosts = self.hosts()?;
        hosts.insert(name.to_owned(), host.clone());
        self.write_hosts(&hosts)
    }

    /// Forgets the host called `name`, answering whether there was one.
    pub fn forget_host(&self, name: &str) -> Result<Option<Host>, String> {
        let mut hosts = self.hosts()?;
        let gone = hosts.remove(name);
        if gone.is_some() {
            self.write_hosts(&hosts)?;
        }
        Ok(gone)
    }

    fn write_hosts(&self, hosts: &BTreeMap<String, Host>) -> Result<(), String> {
        let entries: serde_json::Map<String, Value> = hosts
            .iter()
            .map(|(name, host)| {
                (
                    name.clone(),
                    json!({"peer": host.peer.to_string(), "address": host.address}),
                )
            })
            .collect();
        let text = serde_json::to_string_pretty(&json!({ "hosts": entries }))
            .map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let (file, partial) = (self.hosts_file(), self.dir.join("hosts.json.new"));
        std::fs::write(&partial, text)
            .and_then(|()| std::fs::rename(&partial, &file))
            .map_err(|error| format!("the hosts file could not be written: {error}"))
    }

    /// The host a command names: a name from the hosts file, or a peer id in it.
    pub fn host(&self, name: &str) -> Result<Host, String> {
        let hosts = self.hosts()?;
        if let Some(host) = hosts.get(name) {
            return Ok(host.clone());
        }
        hosts
            .values()
            .find(|host| host.peer.to_string() == name)
            .cloned()
            .ok_or_else(|| format!("no paired host is called {name}: see `cabuya hosts`"))
    }
}

fn from_environment() -> Result<PathBuf, String> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(dir) = var("CABUYA_HOME") {
        return Ok(PathBuf::from(dir));
    }
    if cfg!(windows) {
        if let Some(dir) = var("APPDATA") {
            return Ok(PathBuf::from(dir).join("cabuya"));
        }
    } else if let Some(dir) = var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("cabuya"));
    } else if let Some(dir) = var("HOME") {
        return Ok(PathBuf::from(dir).join(".config").join("cabuya"));
    }
    Err("no folder for the keys: give --home or set CABUYA_HOME".to_owned())
}
