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
