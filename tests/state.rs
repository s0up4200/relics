//! The app state file in `$XDG_STATE_HOME/silo-omarchy/`.

use std::path::PathBuf;

use silo_omarchy::state::{SavedAccount, SavedServer, State};

fn empty_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("silo-omarchy-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_first_load_makes_one_device_id_for_the_install() {
    let dir = empty_dir("device");
    let first = State::load(&dir).unwrap();
    assert_eq!(first.device_id.len(), 36, "{}", first.device_id);
    assert_eq!(State::load(&dir).unwrap().device_id, first.device_id);
    assert_ne!(State::load(&empty_dir("device-other")).unwrap().device_id, first.device_id);
}

#[test]
fn servers_accounts_and_the_last_profile_survive_a_restart() {
    let dir = empty_dir("roundtrip");
    let mut state = State::load(&dir).unwrap();
    state.remember(
        SavedServer { id: "server-1".into(), url: "https://silo.test/media/".into(), accounts: Vec::new() },
        SavedAccount { id: "1".into(), username: "laura".into(), last_profile: None },
    );
    state.active_account_mut().unwrap().last_profile = Some("p-owner".into());
    state.save(&dir).unwrap();

    let state = State::load(&dir).unwrap();
    let (server, account) = state.active().unwrap();
    assert_eq!(server.id, "server-1");
    assert_eq!(server.url, "https://silo.test/media/");
    assert_eq!(account.username, "laura");
    assert_eq!(account.last_profile.as_deref(), Some("p-owner"));
}

#[test]
fn signing_in_again_keeps_one_entry_for_the_server_and_the_account() {
    let dir = empty_dir("again");
    let mut state = State::load(&dir).unwrap();
    for url in ["https://old.test/", "https://new.test/"] {
        state.remember(
            SavedServer { id: "server-1".into(), url: url.into(), accounts: Vec::new() },
            SavedAccount { id: "1".into(), username: "laura".into(), last_profile: None },
        );
    }
    state.remember(
        SavedServer { id: "server-1".into(), url: "https://new.test/".into(), accounts: Vec::new() },
        SavedAccount { id: "2".into(), username: "kim".into(), last_profile: None },
    );
    assert_eq!(state.servers.len(), 1);
    assert_eq!(state.servers[0].url, "https://new.test/");
    assert_eq!(state.servers[0].accounts.len(), 2);
    assert_eq!(state.active().unwrap().1.username, "kim");
}

#[test]
fn the_server_list_has_one_entry_for_each_server_and_account_pair() {
    let mut state = State::fresh();
    for (server, url, account, username) in [
        ("server-1", "https://silo.test/media/", "1", "laura"),
        ("server-2", "http://10.0.0.5:8080/", "1", "laura"),
        ("server-1", "https://silo.test/media/", "2", "kim"),
    ] {
        state.remember(
            SavedServer { id: server.into(), url: url.into(), accounts: Vec::new() },
            SavedAccount { id: account.into(), username: username.into(), last_profile: None },
        );
    }
    let pairs: Vec<_> =
        state.pairs().into_iter().map(|p| (p.server_id, p.account_id, p.server_name, p.username)).collect();
    let pair = |s: &str, a: &str, n: &str, u: &str| (s.to_string(), a.to_string(), n.to_string(), u.to_string());
    assert_eq!(
        pairs,
        [
            pair("server-1", "1", "silo.test", "laura"),
            pair("server-1", "2", "silo.test", "kim"),
            pair("server-2", "1", "10.0.0.5", "laura"),
        ]
    );
}

#[test]
fn remove_drops_the_account_and_a_server_with_no_accounts_left() {
    let mut state = State::fresh();
    for (server, account) in [("server-1", "1"), ("server-1", "2"), ("server-2", "1")] {
        state.remember(
            SavedServer { id: server.into(), url: "https://silo.test/".into(), accounts: Vec::new() },
            SavedAccount { id: account.into(), username: "laura".into(), last_profile: None },
        );
    }
    state.remove("server-1", "1");
    assert_eq!(state.servers.len(), 2);
    assert_eq!(state.active, Some(("server-2".into(), "1".into())), "another pair stays active");

    state.remove("server-2", "1");
    assert_eq!(state.servers.len(), 1, "the Server with no Accounts left goes");
    assert_eq!(state.active, Some(("server-1".into(), "2".into())), "the first pair left becomes active");

    state.remove("server-1", "2");
    assert!(state.servers.is_empty());
    assert_eq!(state.active, None);
}
