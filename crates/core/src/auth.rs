//! Microsoft account sign-in (device code flow) → Xbox Live → Minecraft.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{http, Event, Launcher};

const MS_DEVICE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const MS_TOKEN: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_AUTH: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE: &str = "https://api.minecraftservices.com/minecraft/profile";
const SCOPE: &str = "XboxLive.signin offline_access";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Minecraft profile UUID (no dashes).
    pub id: String,
    pub name: String,
    /// Kept in the OS credential store; only written to the file as a fallback.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mc_token: String,
    pub mc_expires_at: u64,
    /// True when the tokens live in the credential store, not this file.
    #[serde(default)]
    pub tokens_in_keyring: bool,
}

/// What the UI and API see (no tokens).
#[derive(Debug, Clone, Serialize)]
pub struct AccountInfo {
    pub id: String,
    pub name: String,
    pub selected: bool,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn refresh_key(id: &str) -> String {
    format!("{id}:refresh")
}
fn mc_key(id: &str) -> String {
    format!("{id}:minecraft")
}

/// Load accounts with their tokens filled in from the credential store.
/// Accounts whose tokens are still in the file (older versions, or a store
/// that was unavailable) are moved into the store on the way.
pub async fn load_accounts(l: &Launcher) -> Vec<Account> {
    let mut accounts: Vec<Account> = http::read_json(&l.paths.accounts_file()).await.unwrap_or_default();
    let mut needs_migration = false;
    for a in &mut accounts {
        if a.tokens_in_keyring {
            let id = a.id.clone();
            let (r, m) = tokio::task::spawn_blocking(move || {
                (crate::secrets::get(&refresh_key(&id)), crate::secrets::get(&mc_key(&id)))
            })
            .await
            .unwrap_or((Ok(None), Ok(None)));
            a.refresh_token = r.ok().flatten().unwrap_or_default();
            a.mc_token = m.ok().flatten().unwrap_or_default();
            if a.mc_token.is_empty() {
                a.mc_expires_at = 0; // force a refresh
            }
        } else if !a.refresh_token.is_empty() {
            needs_migration = true;
        }
    }
    if needs_migration {
        let _ = save_accounts(l, &accounts).await;
    }
    accounts
}

async fn save_accounts(l: &Launcher, accounts: &[Account]) -> Result<()> {
    let mut on_disk = Vec::with_capacity(accounts.len());
    for a in accounts {
        let (id, r, m) = (a.id.clone(), a.refresh_token.clone(), a.mc_token.clone());
        let stored = tokio::task::spawn_blocking(move || -> Result<()> {
            crate::secrets::set(&refresh_key(&id), &r)?;
            crate::secrets::set(&mc_key(&id), &m)?;
            Ok(())
        })
        .await?;
        let mut d = a.clone();
        match stored {
            Ok(()) => {
                d.refresh_token.clear();
                d.mc_token.clear();
                d.tokens_in_keyring = true;
            }
            Err(e) => {
                // Fall back to the file so sign-in still works.
                eprintln!("{e:#}; keeping tokens for {} in accounts.json", a.name);
                d.tokens_in_keyring = false;
            }
        }
        on_disk.push(d);
    }
    http::write_json(&l.paths.accounts_file(), &on_disk).await
}

pub async fn list(l: &Launcher) -> Vec<AccountInfo> {
    let selected = l.config.read().await.selected_account.clone();
    load_accounts(l)
        .await
        .into_iter()
        .map(|a| AccountInfo {
            selected: selected.as_deref() == Some(&a.id),
            id: a.id,
            name: a.name,
        })
        .collect()
}

pub async fn remove(l: &Launcher, id: &str) -> Result<()> {
    let mut accounts = load_accounts(l).await;
    accounts.retain(|a| a.id != id);
    save_accounts(l, &accounts).await?;
    let id = id.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        crate::secrets::delete(&refresh_key(&id));
        crate::secrets::delete(&mc_key(&id));
    })
    .await;
    l.emit(Event::AccountsChanged);
    Ok(())
}

async fn client_id(l: &Launcher) -> Result<String> {
    l.config.read().await.ms_client_id.clone().ok_or_else(|| {
        anyhow!(
            "No Microsoft client ID is set. Add your Azure app's client ID in Settings \
             (see the README for how to register one)."
        )
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    #[serde(default)]
    pub message: String,
}

/// Start a sign-in. Returns the code to show the user; the rest happens in
/// the background and ends with `AccountsChanged` or `LoginFailed`.
pub async fn begin_login(l: &Launcher) -> Result<DeviceCode> {
    let cid = client_id(l).await?;
    let resp = l
        .http
        .post(MS_DEVICE)
        .form(&[("client_id", cid.as_str()), ("scope", SCOPE)])
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!(
            "Microsoft rejected the sign-in request ({}). Check the client ID in Settings.",
            resp.status()
        );
    }
    let code: DeviceCode = resp.json().await?;
    l.emit(Event::LoginCode {
        user_code: code.user_code.clone(),
        verification_uri: code.verification_uri.clone(),
        message: code.message.clone(),
    });
    let l2 = l.clone();
    let c2 = code.clone();
    tokio::spawn(async move {
        if let Err(e) = finish_login(&l2, &cid, &c2).await {
            l2.emit(Event::LoginFailed { error: format!("{e:#}") });
        }
    });
    Ok(code)
}

#[derive(Deserialize)]
struct MsToken {
    access_token: String,
    refresh_token: String,
}

#[derive(Deserialize)]
struct MsError {
    error: String,
    #[serde(default)]
    error_description: String,
}

async fn finish_login(l: &Launcher, cid: &str, code: &DeviceCode) -> Result<()> {
    let deadline = now() + code.expires_in;
    let mut interval = code.interval.max(1);
    let token = loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if now() > deadline {
            bail!("The sign-in code expired. Try again.");
        }
        let resp = l
            .http
            .post(MS_TOKEN)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", cid),
                ("device_code", code.device_code.as_str()),
            ])
            .send()
            .await?;
        if resp.status().is_success() {
            break resp.json::<MsToken>().await?;
        }
        let err: MsError = resp.json().await.unwrap_or(MsError {
            error: "unknown".into(),
            error_description: String::new(),
        });
        match err.error.as_str() {
            "authorization_pending" => continue,
            "slow_down" => interval += 5,
            "authorization_declined" => bail!("Sign-in was declined."),
            "expired_token" => bail!("The sign-in code expired. Try again."),
            _ => bail!("Microsoft sign-in failed: {} {}", err.error, err.error_description),
        }
    };
    let account = minecraft_from_ms(l, &token.access_token, token.refresh_token).await?;
    let mut accounts = load_accounts(l).await;
    accounts.retain(|a| a.id != account.id);
    let id = account.id.clone();
    accounts.push(account);
    save_accounts(l, &accounts).await?;
    {
        let mut cfg = l.config.write().await;
        cfg.selected_account = Some(id);
        cfg.save(&l.paths).await?;
    }
    l.emit(Event::AccountsChanged);
    Ok(())
}

async fn minecraft_from_ms(l: &Launcher, ms_access: &str, refresh: String) -> Result<Account> {
    // Xbox Live
    let xbl: serde_json::Value = post_json(
        l,
        XBL_AUTH,
        json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access}")
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }),
    )
    .await
    .context("Xbox Live sign-in failed")?;
    let xbl_token = xbl["Token"].as_str().ok_or_else(|| anyhow!("no Xbox Live token"))?;
    let uhs = xbl["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .ok_or_else(|| anyhow!("no Xbox user hash"))?
        .to_string();

    // XSTS
    let resp = l
        .http
        .post(XSTS_AUTH)
        .json(&json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }))
        .send()
        .await?;
    if resp.status().as_u16() == 401 {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let msg = match body["XErr"].as_u64() {
            Some(2148916233) => "This Microsoft account has no Xbox profile. Sign in once at minecraft.net to create one.",
            Some(2148916235) => "Xbox Live is not available in your country.",
            Some(2148916236) | Some(2148916237) => "This account needs adult verification on the Xbox website.",
            Some(2148916238) => "This is a child account; an adult must add it to a Microsoft family first.",
            _ => "Xbox Live refused the sign-in.",
        };
        bail!(msg);
    }
    if !resp.status().is_success() {
        bail!("XSTS sign-in failed ({})", resp.status());
    }
    let xsts: serde_json::Value = resp.json().await?;
    let xsts_token = xsts["Token"].as_str().ok_or_else(|| anyhow!("no XSTS token"))?;

    // Minecraft
    let mc: serde_json::Value = post_json(
        l,
        MC_LOGIN,
        json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") }),
    )
    .await
    .context(
        "Minecraft rejected the sign-in. New Azure apps must be approved by Mojang \
         for the Minecraft API before sign-in works; see the README.",
    )?;
    let mc_token = mc["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("no Minecraft token"))?
        .to_string();
    let expires_in = mc["expires_in"].as_u64().unwrap_or(86400);

    let resp = l.http.get(MC_PROFILE).bearer_auth(&mc_token).send().await?;
    if resp.status().as_u16() == 404 {
        bail!("This Microsoft account does not own Minecraft: Java Edition.");
    }
    if !resp.status().is_success() {
        bail!("Could not read the Minecraft profile ({})", resp.status());
    }
    let profile: serde_json::Value = resp.json().await?;
    Ok(Account {
        id: profile["id"].as_str().unwrap_or_default().to_string(),
        name: profile["name"].as_str().unwrap_or_default().to_string(),
        refresh_token: refresh,
        mc_token,
        mc_expires_at: now() + expires_in,
        tokens_in_keyring: false,
    })
}

async fn post_json(l: &Launcher, url: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    let resp = l
        .http
        .post(url)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    Ok(resp.json().await?)
}

/// Return a usable account, refreshing its Minecraft token if needed.
pub async fn usable_account(l: &Launcher, id: Option<&str>) -> Result<Account> {
    let wanted = match id {
        Some(i) => Some(i.to_string()),
        None => l.config.read().await.selected_account.clone(),
    };
    let mut accounts = load_accounts(l).await;
    let idx = accounts
        .iter()
        .position(|a| Some(&a.id) == wanted.as_ref())
        .or(if accounts.is_empty() { None } else { Some(0) })
        .ok_or_else(|| anyhow!("Sign in with a Microsoft account that owns Minecraft before playing."))?;
    if accounts[idx].mc_expires_at > now() + 300 {
        return Ok(accounts[idx].clone());
    }
    let cid = client_id(l).await?;
    let resp = l
        .http
        .post(MS_TOKEN)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", cid.as_str()),
            ("refresh_token", accounts[idx].refresh_token.as_str()),
            ("scope", SCOPE),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("Your sign-in for {} expired. Sign in again.", accounts[idx].name);
    }
    let tok: MsToken = resp.json().await?;
    let refreshed = minecraft_from_ms(l, &tok.access_token, tok.refresh_token).await?;
    accounts[idx] = refreshed.clone();
    save_accounts(l, &accounts).await?;
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paths;

    fn acct(id: &str) -> Account {
        Account {
            id: id.into(),
            name: "Mio".into(),
            refresh_token: format!("refresh-secret-{id}"),
            mc_token: format!("mc-secret-{id}"),
            mc_expires_at: now() + 3600,
            tokens_in_keyring: false,
        }
    }

    #[tokio::test]
    async fn tokens_stay_out_of_the_file() {
        crate::secrets::use_memory_store();
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        save_accounts(&l, &[acct("aaa")]).await.unwrap();
        let file = std::fs::read_to_string(l.paths.accounts_file()).unwrap();
        assert!(!file.contains("secret"), "{file}");
        let loaded = load_accounts(&l).await;
        assert_eq!(loaded[0].refresh_token, "refresh-secret-aaa");
        assert_eq!(loaded[0].mc_token, "mc-secret-aaa");

        remove(&l, "aaa").await.unwrap();
        assert!(crate::secrets::get("aaa:refresh").unwrap().is_none());
    }

    #[tokio::test]
    async fn migrates_old_plaintext_files() {
        crate::secrets::use_memory_store();
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        // What version 0.1 wrote.
        std::fs::write(
            l.paths.accounts_file(),
            serde_json::to_string(&serde_json::json!([{"id":"bbb","name":"Old","refresh_token":"old-refresh","mc_token":"old-mc","mc_expires_at":1}])).unwrap(),
        )
        .unwrap();
        let loaded = load_accounts(&l).await;
        assert_eq!(loaded[0].refresh_token, "old-refresh");
        let file = std::fs::read_to_string(l.paths.accounts_file()).unwrap();
        assert!(!file.contains("old-refresh") && file.contains("tokens_in_keyring"), "{file}");
        assert_eq!(load_accounts(&l).await[0].mc_token, "old-mc");
    }
}
