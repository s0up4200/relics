//! The app state in `$XDG_STATE_HOME/silo-omarchy/state.json`: the device id, the
//! Servers, the Accounts and the last Profile of each Account. The tokens are not
//! here; they go in the Secret Service.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    /// One random UUID for each install.
    pub device_id: String,
    pub servers: Vec<SavedServer>,
    /// The `(server id, account id)` that the app opens at launch.
    pub active: Option<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedServer {
    /// The `server_id` that the server reports.
    pub id: String,
    pub url: String,
    pub accounts: Vec<SavedAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedAccount {
    pub id: String,
    pub username: String,
    pub last_profile: Option<String>,
}

/// One Server and Account pair, as the Server list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub server_id: String,
    pub account_id: String,
    /// The host of the Server URL.
    pub server_name: String,
    pub username: String,
}

const FILE: &str = "state.json";

impl State {
    /// `$XDG_STATE_HOME/silo-omarchy`, or `~/.local/state/silo-omarchy`.
    pub fn dir() -> PathBuf {
        let base = std::env::var_os("XDG_STATE_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
        base.join("silo-omarchy")
    }

    /// An empty state with a new device id.
    pub fn fresh() -> Self {
        Self { device_id: uuid::Uuid::new_v4().to_string(), servers: Vec::new(), active: None }
    }

    /// Reads the state in `dir`. When there is no file yet, it makes a new device id and saves it.
    pub fn load(dir: &Path) -> io::Result<Self> {
        match std::fs::read(dir.join(FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let state = Self::fresh();
                state.save(dir)?;
                Ok(state)
            }
            Err(err) => Err(err),
        }
    }

    /// Writes the state to a temp file and renames it, so that a crash never leaves half a file.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let temp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&temp, serde_json::to_vec_pretty(self).map_err(io::Error::other)?)?;
        std::fs::rename(temp, dir.join(FILE))
    }

    /// Adds or updates the Server and the Account after a sign-in, and makes them active.
    /// An Account that the state already has keeps its last Profile.
    pub fn remember(&mut self, server: SavedServer, account: SavedAccount) {
        self.active = Some((server.id.clone(), account.id.clone()));
        let index = match self.servers.iter().position(|s| s.id == server.id) {
            Some(index) => {
                self.servers[index].url = server.url;
                index
            }
            None => {
                self.servers.push(server);
                self.servers.len() - 1
            }
        };
        let accounts = &mut self.servers[index].accounts;
        match accounts.iter_mut().find(|a| a.id == account.id) {
            Some(saved) => saved.username = account.username,
            None => accounts.push(account),
        }
    }

    pub fn active(&self) -> Option<(&SavedServer, &SavedAccount)> {
        let (server_id, account_id) = self.active.as_ref()?;
        let server = self.servers.iter().find(|s| &s.id == server_id)?;
        Some((server, server.accounts.iter().find(|a| &a.id == account_id)?))
    }

    /// Each Server and Account pair, in the order the user added them. A second Account on one Server is a
    /// separate pair.
    pub fn pairs(&self) -> Vec<Pair> {
        self.servers
            .iter()
            .flat_map(|server| {
                let name = crate::api::Url::parse(&server.url)
                    .ok()
                    .and_then(|url| url.host_str().map(String::from))
                    .unwrap_or_else(|| server.url.clone());
                server.accounts.iter().map(move |account| Pair {
                    server_id: server.id.clone(),
                    account_id: account.id.clone(),
                    server_name: name.clone(),
                    username: account.username.clone(),
                })
            })
            .collect()
    }

    pub fn active_account_mut(&mut self) -> Option<&mut SavedAccount> {
        let (server_id, account_id) = self.active.as_ref()?;
        let server = self.servers.iter_mut().find(|s| &s.id == server_id)?;
        server.accounts.iter_mut().find(|a| &a.id == account_id)
    }
}
