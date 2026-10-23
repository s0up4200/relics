//! The tokens of each Account in the Secret Service keyring. An error here means that
//! the Secret Service is missing or locked; the caller then keeps the tokens in memory.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use crate::api::Tokens;

const LABEL: &str = "Relics sign-in";

static KEYRING_ON: AtomicBool = AtomicBool::new(true);

/// Turns off the keyring for the whole process. After this call, each load finds nothing, and each save and
/// delete changes nothing and returns Ok. The live-run rig (docs/rig.md) calls it at start.
pub fn turn_off() {
    KEYRING_ON.store(false, Ordering::Relaxed);
}

/// The unlocked keyring, or `None` after [`turn_off`]. Each function in this module opens the keyring here.
async fn keyring() -> Result<Option<oo7::Keyring>> {
    if !KEYRING_ON.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if cfg!(test) {
        panic!("a unit test reached the real keyring");
    }
    let keyring = oo7::Keyring::new().await?;
    keyring.unlock().await?;
    Ok(Some(keyring))
}

fn attributes<'a>(server_id: &'a str, account_id: &'a str) -> Vec<(&'static str, &'a str)> {
    vec![("application", "relics"), ("server_id", server_id), ("account_id", account_id)]
}

/// Fails when the Secret Service is missing or stays locked.
pub async fn check() -> Result<()> {
    keyring().await?;
    Ok(())
}

pub async fn load(server_id: &str, account_id: &str) -> Result<Option<Tokens>> {
    let Some(keyring) = keyring().await? else {
        return Ok(None);
    };
    let Some(item) = keyring.search_items(&attributes(server_id, account_id)).await?.into_iter().next() else {
        return Ok(None);
    };
    item.unlock().await?;
    Ok(Some(serde_json::from_slice(&item.secret().await?)?))
}

pub async fn save(server_id: &str, account_id: &str, tokens: &Tokens) -> Result<()> {
    let Some(keyring) = keyring().await? else {
        return Ok(());
    };
    keyring.create_item(LABEL, &attributes(server_id, account_id), serde_json::to_vec(tokens)?, true).await?;
    Ok(())
}

// PIN tokens (#23). Their `application` value differs from the sign-in items: a search matches each item that
// has the given attributes, so a PIN item with the sign-in attributes plus `profile_id` would match `load`.

fn pin_attributes<'a>(server_id: &'a str, account_id: &'a str, profile_id: &'a str) -> Vec<(&'static str, &'a str)> {
    vec![
        ("application", "relics-pin"),
        ("server_id", server_id),
        ("account_id", account_id),
        ("profile_id", profile_id),
    ]
}

pub async fn load_pin_token(server_id: &str, account_id: &str, profile_id: &str) -> Result<Option<String>> {
    let Some(keyring) = keyring().await? else {
        return Ok(None);
    };
    let attributes = pin_attributes(server_id, account_id, profile_id);
    let Some(item) = keyring.search_items(&attributes).await?.into_iter().next() else {
        return Ok(None);
    };
    item.unlock().await?;
    Ok(Some(String::from_utf8(item.secret().await?.to_vec())?))
}

pub async fn save_pin_token(server_id: &str, account_id: &str, profile_id: &str, token: &str) -> Result<()> {
    let Some(keyring) = keyring().await? else {
        return Ok(());
    };
    let attributes = pin_attributes(server_id, account_id, profile_id);
    keyring.create_item("Relics PIN token", &attributes, token, true).await?;
    Ok(())
}

/// Deletes the sign-in tokens and every PIN token of the Account (#35 sign out).
pub async fn delete(server_id: &str, account_id: &str) -> Result<()> {
    let Some(keyring) = keyring().await? else {
        return Ok(());
    };
    // A search matches each item that has these attributes, so the PIN items of all Profiles go too.
    keyring.delete(&attributes(server_id, account_id)).await?;
    let pins = vec![("application", "relics-pin"), ("server_id", server_id), ("account_id", account_id)];
    keyring.delete(&pins).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn turn_off_keeps_every_call_away_from_the_keyring() {
        turn_off();
        let tokens = Tokens { access_token: "a".into(), refresh_token: "r".into() };
        block_on(async {
            check().await.unwrap();
            assert!(load("server-1", "account-1").await.unwrap().is_none());
            save("server-1", "account-1", &tokens).await.unwrap();
            assert!(load_pin_token("server-1", "account-1", "p").await.unwrap().is_none());
            save_pin_token("server-1", "account-1", "p", "t").await.unwrap();
            delete("server-1", "account-1").await.unwrap();
        });
    }
}
