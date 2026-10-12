//! The tokens of each Account in the Secret Service keyring. An error here means that
//! the Secret Service is missing or locked; the caller then keeps the tokens in memory.

use anyhow::Result;

use crate::api::Tokens;

const LABEL: &str = "silo-omarchy sign-in";

fn attributes<'a>(server_id: &'a str, account_id: &'a str) -> Vec<(&'static str, &'a str)> {
    vec![("application", "silo-omarchy"), ("server_id", server_id), ("account_id", account_id)]
}

/// Fails when the Secret Service is missing or stays locked.
pub async fn check() -> Result<()> {
    oo7::Keyring::new().await?.unlock().await?;
    Ok(())
}

pub async fn load(server_id: &str, account_id: &str) -> Result<Option<Tokens>> {
    let keyring = oo7::Keyring::new().await?;
    keyring.unlock().await?;
    let Some(item) = keyring.search_items(&attributes(server_id, account_id)).await?.into_iter().next() else {
        return Ok(None);
    };
    item.unlock().await?;
    Ok(Some(serde_json::from_slice(&item.secret().await?)?))
}

pub async fn save(server_id: &str, account_id: &str, tokens: &Tokens) -> Result<()> {
    let keyring = oo7::Keyring::new().await?;
    keyring.unlock().await?;
    keyring.create_item(LABEL, &attributes(server_id, account_id), serde_json::to_vec(tokens)?, true).await?;
    Ok(())
}

// PIN tokens (#23). Their `application` value differs from the sign-in items: a search matches each item that
// has the given attributes, so a PIN item with the sign-in attributes plus `profile_id` would match `load`.

fn pin_attributes<'a>(server_id: &'a str, account_id: &'a str, profile_id: &'a str) -> Vec<(&'static str, &'a str)> {
    vec![
        ("application", "silo-omarchy-pin"),
        ("server_id", server_id),
        ("account_id", account_id),
        ("profile_id", profile_id),
    ]
}

pub async fn load_pin_token(server_id: &str, account_id: &str, profile_id: &str) -> Result<Option<String>> {
    let keyring = oo7::Keyring::new().await?;
    keyring.unlock().await?;
    let attributes = pin_attributes(server_id, account_id, profile_id);
    let Some(item) = keyring.search_items(&attributes).await?.into_iter().next() else {
        return Ok(None);
    };
    item.unlock().await?;
    Ok(Some(String::from_utf8(item.secret().await?.to_vec())?))
}

pub async fn save_pin_token(server_id: &str, account_id: &str, profile_id: &str, token: &str) -> Result<()> {
    let keyring = oo7::Keyring::new().await?;
    keyring.unlock().await?;
    let attributes = pin_attributes(server_id, account_id, profile_id);
    keyring.create_item("silo-omarchy PIN token", &attributes, token, true).await?;
    Ok(())
}

/// Deletes the sign-in tokens and every PIN token of the Account (#35 sign out).
pub async fn delete(server_id: &str, account_id: &str) -> Result<()> {
    let keyring = oo7::Keyring::new().await?;
    keyring.unlock().await?;
    // A search matches each item that has these attributes, so the PIN items of all Profiles go too.
    keyring.delete(&attributes(server_id, account_id)).await?;
    let pins = vec![("application", "silo-omarchy-pin"), ("server_id", server_id), ("account_id", account_id)];
    keyring.delete(&pins).await?;
    Ok(())
}
