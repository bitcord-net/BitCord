pub mod commands;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Set while the background auto-unlock task is running so that
/// `get_backend_status` can distinguish "still starting" from
/// "genuinely needs the user to type a passphrase".
static AUTO_UNLOCKING: AtomicBool = AtomicBool::new(false);

use anyhow::Result;
use bitcord_core::{
    api::{
        ApiServer, PushEvent,
        push_broadcaster::{DmEventData, MessageEventData},
        types::DmMessageInfo,
    },
    config::NodeConfig,
    crypto::dm::DmEnvelope,
    identity::{NodeIdentity, keystore::KeyStore},
    network::{NodeAddr, client::NodeClient, protocol::NodePush},
    node::{NodeInitConfig, init_node, store::NodeStore},
};
use chrono::Utc;
use commands::NodeState;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(desktop)]
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tracing::{info, warn};

#[cfg(not(mobile))]
const KEYCHAIN_SERVICE: &str = "com.bitcord.desktop";
#[cfg(not(mobile))]
const KEYCHAIN_KEY: &str = "identity_passphrase";

#[cfg(mobile)]
mod mobile_keychain {
    use serde::{Deserialize, Serialize};
    use tauri::{
        Manager, Runtime,
        plugin::{Builder, TauriPlugin},
    };

    pub struct PassphraseHandle<R: Runtime>(tauri::plugin::PluginHandle<R>);

    #[derive(Deserialize)]
    struct GetResult {
        value: Option<String>,
    }

    #[derive(Serialize)]
    struct SaveArgs<'a> {
        value: &'a str,
    }

    impl<R: Runtime> PassphraseHandle<R> {
        pub fn get(&self) -> Option<String> {
            self.0
                .run_mobile_plugin::<GetResult>("getPassphrase", ())
                .ok()
                .and_then(|r| r.value)
        }

        pub fn save(&self, value: &str) {
            let _ = self
                .0
                .run_mobile_plugin::<serde_json::Value>("savePassphrase", SaveArgs { value });
        }

        pub fn delete(&self) {
            let _ = self
                .0
                .run_mobile_plugin::<serde_json::Value>("deletePassphrase", ());
        }
    }

    pub fn init<R: Runtime>() -> TauriPlugin<R> {
        Builder::new("passphrase")
            .setup(|app, api| {
                let handle =
                    api.register_android_plugin("net.bitcord.client", "PassphrasePlugin")?;
                app.manage(PassphraseHandle(handle));
                Ok(())
            })
            .build()
    }
}

fn keychain_get(_app: &AppHandle) -> Option<String> {
    #[cfg(mobile)]
    return _app
        .state::<mobile_keychain::PassphraseHandle<tauri::Wry>>()
        .get();

    #[cfg(not(mobile))]
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_KEY)
        .ok()
        .and_then(|e| e.get_password().ok())
}

fn keychain_save(_app: &AppHandle, password: &str) {
    #[cfg(mobile)]
    {
        _app.state::<mobile_keychain::PassphraseHandle<tauri::Wry>>()
            .save(password);
        return;
    }
    #[cfg(not(mobile))]
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_KEY) {
        if let Err(e) = entry.set_password(password) {
            warn!("Failed to set passphrase in keychain: {e}");
        }
    }
}

fn keychain_delete(_app: &AppHandle) {
    #[cfg(mobile)]
    {
        _app.state::<mobile_keychain::PassphraseHandle<tauri::Wry>>()
            .delete();
        return;
    }
    #[cfg(not(mobile))]
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_KEY) {
        let _ = entry.delete_credential();
    }
}

fn get_config_path(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_data_dir()
        .expect("failed to resolve app data dir")
        .join("config.toml")
}

// ── Tauri commands ────────────────────────────────────────────────────────────

/// Open a native file picker and return the selected path, or an error string
/// if the user cancels or the dialog fails.
#[tauri::command]
async fn pick_file(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_file(move |path| {
        let _ = tx.send(path);
    });
    rx.await
        .map_err(|_| "Dialog closed unexpectedly".to_string())?
        .map(|p| p.to_string())
        .ok_or_else(|| "No file selected".to_string())
}

// ── Backend initialisation ────────────────────────────────────────────────────

/// Determine whether the app needs onboarding, unlocking, or can auto-start.
///
/// Emits one of:
///  - `backend_status { status: "first_run" }` — identity file does not exist
///  - `backend_status { status: "needs_unlock" }` — identity exists, passphrase required
///  - then proceeds to call `start_backend_with_passphrase` if auto-unlock succeeds
async fn check_backend_status(app_handle: AppHandle) {
    let config_path = get_config_path(&app_handle);
    info!("Reading config from {:?}", config_path);
    let config = NodeConfig::load_or_default(&config_path).unwrap_or_default();
    info!(
        "Config loaded: save_passphrase={}, identity_path={:?}",
        config.save_passphrase, config.identity_path
    );

    if !config.identity_path.exists() {
        info!("First run — waiting for onboarding");
        let _ = app_handle.emit(
            "backend_status",
            serde_json::json!({ "status": "first_run" }),
        );
        return;
    }

    // Try auto-unlock from OS keychain if save_passphrase is enabled.
    if config.save_passphrase {
        info!("save_passphrase enabled, attempting auto-unlock");
        AUTO_UNLOCKING.store(true, Ordering::SeqCst);
        match keychain_get(&app_handle) {
            Some(passphrase) => {
                info!("Auto-unlocking from OS keychain");
                match start_backend_with_passphrase(app_handle.clone(), &passphrase).await {
                    Ok(()) => {
                        AUTO_UNLOCKING.store(false, Ordering::SeqCst);
                        return;
                    }
                    Err(e) => {
                        warn!("Keychain passphrase failed to start backend: {e}; prompting user");
                        // Fall through to needs_unlock
                    }
                }
            }
            None => {
                warn!("No passphrase found in keychain");
                // Fall through to needs_unlock
            }
        }
        AUTO_UNLOCKING.store(false, Ordering::SeqCst);
    } else {
        info!("save_passphrase disabled, skipping auto-unlock");
    }

    info!("Identity found — waiting for passphrase");
    let _ = app_handle.emit(
        "backend_status",
        serde_json::json!({ "status": "needs_unlock" }),
    );
}

/// Query the current backend status — called by the frontend on mount to ensure
/// it doesn't miss the initial "first_run" or "needs_unlock" event.
#[tauri::command]
async fn get_backend_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let config_path = get_config_path(&app);
    let config = NodeConfig::load_or_default(&config_path).unwrap_or_default();

    if !config.identity_path.exists() {
        return Ok(serde_json::json!({
            "status": "first_run",
            "save_passphrase_enabled": config.save_passphrase
        }));
    }

    // Check if NodeState exists (meaning backend has started)
    if app.try_state::<NodeState>().is_some() {
        return Ok(serde_json::json!({
            "status": "ready",
            "save_passphrase_enabled": config.save_passphrase
        }));
    }

    // Auto-unlock is still in progress — tell the frontend to wait
    // instead of showing the password prompt.
    if AUTO_UNLOCKING.load(Ordering::SeqCst) {
        return Ok(serde_json::json!({
            "status": "auto_unlocking",
            "save_passphrase_enabled": config.save_passphrase
        }));
    }

    Ok(serde_json::json!({
        "status": "needs_unlock",
        "save_passphrase_enabled": config.save_passphrase
    }))
}

/// Unlock an existing identity and start the full backend.
#[tauri::command]
async fn unlock_identity(
    app: AppHandle,
    passphrase: String,
    save_passphrase: bool,
) -> Result<(), String> {
    start_backend_with_passphrase(app.clone(), &passphrase)
        .await
        .map_err(|e| e.to_string())?;

    // Persist or clear keychain entry based on user preference.
    handle_save_passphrase(&app, &passphrase, save_passphrase).await;

    // Update config with save_passphrase preference.
    let config_path = get_config_path(&app);
    if let Ok(mut config) = NodeConfig::load_or_default(&config_path) {
        config.save_passphrase = save_passphrase;
        let _ = config.save(&config_path);
    }

    Ok(())
}

/// Create a new identity with the given passphrase and start the backend.
#[tauri::command]
async fn create_identity(
    app: AppHandle,
    passphrase: String,
    display_name: String,
    save_passphrase: bool,
) -> Result<(), String> {
    let config_path = get_config_path(&app);
    let mut config = NodeConfig::load_or_default(&config_path).map_err(|e| e.to_string())?;

    // Generate and save the identity with the real passphrase.
    let identity = NodeIdentity::generate();
    KeyStore::save(&config.identity_path, &identity, &passphrase)
        .map_err(|e| format!("failed to save identity: {e}"))?;

    // Persist display name.
    config.display_name = Some(display_name);
    config.save_passphrase = save_passphrase;
    let _ = config.save(&config_path);

    start_backend_with_passphrase(app.clone(), &passphrase)
        .await
        .map_err(|e| e.to_string())?;

    handle_save_passphrase(&app, &passphrase, save_passphrase).await;

    Ok(())
}

/// Query whether the current config has save_passphrase enabled.
#[tauri::command]
async fn get_save_passphrase(app: AppHandle) -> Result<bool, String> {
    let config_path = get_config_path(&app);
    let config = NodeConfig::load_or_default(&config_path).unwrap_or_default();
    Ok(config.save_passphrase)
}

/// Update the save_passphrase preference and store/clear the keychain entry.
#[tauri::command]
async fn set_save_passphrase(
    app: AppHandle,
    enabled: bool,
    passphrase: String,
) -> Result<(), String> {
    let config_path = get_config_path(&app);
    let config = NodeConfig::load_or_default(&config_path).unwrap_or_default();

    // Verify the passphrase is correct before storing it.
    if enabled {
        KeyStore::load(&config.identity_path, &passphrase)
            .map_err(|_| "Invalid passphrase".to_string())?;
    }

    if let Ok(mut cfg) = NodeConfig::load_or_default(&config_path) {
        cfg.save_passphrase = enabled;
        let _ = cfg.save(&config_path);
    }
    handle_save_passphrase(&app, &passphrase, enabled).await;
    Ok(())
}

async fn handle_save_passphrase(app: &AppHandle, passphrase: &str, save: bool) {
    if save {
        info!("Saving passphrase to OS keychain");
        keychain_save(app, passphrase);
    } else {
        info!("Removing passphrase from OS keychain");
        keychain_delete(app);
    }
}

/// Full backend bootstrap — called once with the correct passphrase.
async fn start_backend_with_passphrase(app_handle: AppHandle, passphrase: &str) -> Result<()> {
    let config_path = get_config_path(&app_handle);
    let config = NodeConfig::load_or_default(&config_path)?;

    info!("BitCord backend starting");

    // ── Identity ─────────────────────────────────────────────────────────────
    let identity = if config.identity_path.exists() {
        KeyStore::load(&config.identity_path, passphrase)?
    } else {
        let id = NodeIdentity::generate();
        KeyStore::save(&config.identity_path, &id, passphrase)?;
        id
    };
    let identity = std::sync::Arc::new(identity);

    let data_dir = config.data_dir.clone();
    let is_server = config.node_mode != bitcord_core::config::NodeMode::GossipClient;

    // ── Shared node initialization ────────────────────────────────────────────
    let result = init_node(NodeInitConfig {
        identity: std::sync::Arc::clone(&identity),
        passphrase: if passphrase.is_empty() {
            None
        } else {
            Some(passphrase.to_owned())
        },
        config,
        config_path,
        join_password: None,
        fallback_to_random_port: true,
        dht_self_addr: None,
        store_db_path: data_dir.join("node.redb"),
    })
    .await?;

    // Keep the metrics sender alive (Tauri manages it so commands can send updates).
    app_handle.manage(result.metrics_tx);

    // ── NodeClient connection to embedded server ───────────────────────────────
    let node_state_local_client = if is_server {
        let actual_port = result
            .quic_port
            .expect("QUIC server must be running in Peer/HeadlessSeed mode");
        let cert_fingerprint = result
            .cert_fingerprint
            .expect("QUIC server must be running in Peer/HeadlessSeed mode");
        let local_node_addr = NodeAddr::new("127.0.0.1".parse().unwrap(), actual_port);
        let (local_client, _local_node_pk, push_rx) = NodeClient::connect(
            local_node_addr,
            cert_fingerprint,
            std::sync::Arc::clone(&identity),
        )
        .await
        .map_err(|e| anyhow::anyhow!("connect NodeClient to embedded node: {e}"))?;
        info!("NodeClient connected to embedded QUIC node");
        Some((local_client, push_rx))
    } else {
        None
    };

    let app_state = result.app_state;
    let store = app_state.node_store.clone().expect("node_store always set");

    // ── Background cache retention janitor ────────────────────────────────────
    let storage_limit_mb = app_state.config.read().await.storage_limit_mb;
    let is_headless_seed =
        app_state.config.read().await.node_mode == bitcord_core::config::NodeMode::HeadlessSeed;
    if !is_headless_seed {
        let store_for_retention = std::sync::Arc::clone(&store);
        let max_bytes = storage_limit_mb * 1_048_576;
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // skip the immediate first tick
            loop {
                interval.tick().await;
                enforce_cache_retention(&store_for_retention, max_bytes).await;
            }
        });
    }

    // ── JSON-RPC API server ───────────────────────────────────────────────────
    let api_addr: std::net::SocketAddr = "127.0.0.1:7331".parse().expect("valid socket addr");
    let api_handle = ApiServer::start(api_addr, std::sync::Arc::clone(&app_state)).await?;
    info!(addr = %api_handle.local_addr(), "API server ready");
    app_handle.manage(api_handle);

    // ── NodeState (Tauri managed) ─────────────────────────────────────────────
    let channel_keys = std::sync::Arc::clone(&app_state.channel_keys);
    let communities_map = std::sync::Arc::clone(&app_state.communities);

    let (local_client_opt, push_rx_opt) = match node_state_local_client {
        Some((client, push_rx)) => (Some(client), Some(push_rx)),
        None => (None, None),
    };

    let node_state = NodeState {
        identity: Arc::clone(&identity),
        local_client: tokio::sync::Mutex::new(local_client_opt),
        remote_clients: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        channel_keys,
        communities: communities_map,
    };
    app_handle.manage(node_state);

    // ── Push relay task (only when server is running) ─────────────────────────
    // Forwards NodePush events from the embedded node to:
    //  1. Tauri frontend events  (app_handle.emit)
    //  2. JSON-RPC PushBroadcaster (existing frontend path)
    if let Some(mut push_rx) = push_rx_opt {
        let app_for_push = app_handle.clone();
        let state_for_push = Arc::clone(&app_state);
        tauri::async_runtime::spawn(async move {
            while let Some(push) = push_rx.recv().await {
                match push {
                    NodePush::NewMessage { channel_id, entry } => {
                        // Emit Tauri event for frontend listeners.
                        let _ = app_for_push.emit(
                            "message:new",
                            serde_json::json!({
                                "channel_id": channel_id.to_string(),
                                "seq": entry.seq,
                                "author_id": entry.author_id,
                                "timestamp_ms": entry.timestamp_ms,
                            }),
                        );

                        // Also bridge to the JSON-RPC broadcaster so existing
                        // WebSocket subscribers receive the event.
                        let (community_id, author_name) = {
                            let channels = state_for_push.channels.read().await;
                            let cid = channels
                                .get(&channel_id.to_string())
                                .map(|c| c.community_id.to_string())
                                .unwrap_or_default();

                            let members = state_for_push.members.read().await;
                            let name = members.get(&cid).and_then(|list| {
                                list.get(&entry.author_id).map(|m| m.display_name.clone())
                            });
                            (cid, name)
                        };
                        state_for_push
                            .broadcaster
                            .send(PushEvent::MessageNew(MessageEventData {
                                message_id: entry.message_id.clone(),
                                channel_id: channel_id.to_string(),
                                community_id,
                                author_id: entry.author_id.clone(),
                                author_name,
                                timestamp: chrono::DateTime::from_timestamp_millis(
                                    entry.timestamp_ms,
                                )
                                .unwrap_or_else(Utc::now),
                                body: None,
                            }));
                    }
                    NodePush::NewDm { entry, .. } => {
                        // Attempt to decrypt the DM envelope (ciphertext = postcard DmEnvelope).
                        // If decryption succeeds this message is addressed to us; relay it via
                        // the RPC broadcaster so the frontend's dm_new subscription fires.
                        let sk_bytes = {
                            let signing_key = &state_for_push.signing_key;
                            signing_key.to_bytes()
                        };
                        let x25519_sk =
                            NodeIdentity::from_signing_key_bytes(&sk_bytes).x25519_secret();
                        if let Ok(envelope) = postcard::from_bytes::<DmEnvelope>(&entry.ciphertext)
                        {
                            if let Ok(plaintext_bytes) = envelope.open(&x25519_sk) {
                                let (body, reply_to, payload_id) = match postcard::from_bytes::<
                                    bitcord_core::crypto::dm::DmPayload,
                                >(
                                    &plaintext_bytes
                                ) {
                                    Ok(p) => {
                                        let id = if p.id.is_empty() { None } else { Some(p.id) };
                                        (p.body, p.reply_to, id)
                                    }
                                    Err(_) => match String::from_utf8(plaintext_bytes) {
                                        Ok(s) => (s, None, None),
                                        Err(_) => {
                                            // Not valid UTF-8 either; skip.
                                            continue;
                                        }
                                    },
                                };
                                let message_id =
                                    payload_id.unwrap_or_else(|| entry.message_id.clone());
                                let timestamp =
                                    chrono::DateTime::from_timestamp_millis(entry.timestamp_ms)
                                        .unwrap_or_else(Utc::now);
                                let msg = DmMessageInfo {
                                    id: message_id,
                                    peer_id: entry.author_id.clone(),
                                    author_id: entry.author_id.clone(),
                                    timestamp,
                                    body,
                                    reply_to,
                                    edited_at: None,
                                };
                                // Persist to local DM store.
                                {
                                    let mut dms = state_for_push.dms.write().await;
                                    dms.entry(entry.author_id.clone())
                                        .or_default()
                                        .push(msg.clone());
                                }
                                // Fire the RPC push so the frontend dm_new subscription
                                // receives the full message.
                                state_for_push
                                    .broadcaster
                                    .send(PushEvent::DmNew(DmEventData { message: msg }));
                            }
                        }
                        // Always emit the lightweight Tauri event.
                        let _ = app_for_push.emit(
                            "dm:new",
                            serde_json::json!({
                                "seq": entry.seq,
                                "author_id": entry.author_id,
                                "timestamp_ms": entry.timestamp_ms,
                            }),
                        );
                    }
                    NodePush::Presence { user_pk, status } => {
                        let pk_b58 = bs58::encode(&user_pk).into_string();
                        let _ = app_for_push.emit(
                            "presence:changed",
                            serde_json::json!({ "user_pk": pk_b58, "status": status }),
                        );
                    }
                    // GossipMessage pushes are forwarded from remote peers through
                    // the NetworkHandle push-reader; the local embedded-node push
                    // relay doesn't need to act on them.
                    NodePush::GossipMessage { .. } => {}
                }
            }
            warn!("push relay task exited — NodeClient push stream closed");
        });
    }

    let _ = app_handle.emit("backend_status", serde_json::json!({ "status": "ready" }));
    Ok(())
}

// ── Cache retention ───────────────────────────────────────────────────────────

/// Enforce the configured storage limit across every stored channel.
///
/// Called periodically (every 5 minutes) by the background janitor task.
/// Seed nodes are exempt — they are always-on and should retain all data.
/// For regular embedded nodes the limit is divided equally among all
/// known channels so no single channel starves others.
async fn enforce_cache_retention(store: &Arc<NodeStore>, max_total_bytes: u64) {
    let channel_ids = match store.all_channel_ids() {
        Ok(ids) => ids,
        Err(e) => {
            warn!("cache retention: failed to list channels: {e}");
            return;
        }
    };
    if channel_ids.is_empty() {
        return;
    }
    let n = channel_ids.len() as u64;
    let per_channel_limit = (max_total_bytes / n).max(1);
    for (community_pk, channel_id) in &channel_ids {
        if let Err(e) = store.enforce_retention(community_pk, channel_id, per_channel_limit) {
            warn!(channel = %channel_id, "cache retention: enforce failed: {e}");
        }
    }
    info!(
        channels = n,
        limit_mb = max_total_bytes / 1_048_576,
        per_channel_kb = per_channel_limit / 1024,
        "cache retention pass complete"
    );
}

// ── System tray ───────────────────────────────────────────────────────────────

#[cfg(desktop)]
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show_hide = MenuItemBuilder::new("Show / Hide")
        .id("show_hide")
        .build(app)?;
    let quit = MenuItemBuilder::new("Quit BitCord").id("quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show_hide)
        .separator()
        .item(&quit)
        .build()?;

    let mut tray_builder = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("BitCord")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show_hide" => toggle_window(app),
            "quit" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.destroy();
                }

                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        tray_builder = tray_builder.icon(icon.clone());
    }

    tray_builder.build(app)?;
    Ok(())
}

#[cfg(desktop)]
fn toggle_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            show_window(app);
        }
    }
}

#[cfg(desktop)]
fn show_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

// ── App entry point ───────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ── 1. Init tracing (static for now) ───────────────────────────────────────
    // Read config at very early boot to determine the log level.
    let log_level = {
        // Match Tauri's app_data_dir() behavior:
        // - Linux:   ~/.local/share/net.bitcord.client/
        // - macOS:   ~/Library/Application Support/net.bitcord.client/
        // - Windows: %APPDATA%\net.bitcord.client\
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA")
                .map(|p| std::path::PathBuf::from(p).join("net.bitcord.client"))
        } else {
            // Match Tauri's app_data_dir() on Linux/macOS:
            // Linux:   ~/.local/share/net.bitcord.client/
            // macOS:   ~/Library/Application Support/net.bitcord.client/
            directories::BaseDirs::new().map(|d| {
                let mut p = d.data_dir().to_path_buf();
                if cfg!(target_os = "macos") {
                    p.push("Application Support");
                }
                p.push("net.bitcord.client");
                p
            })
        };

        let config_path = base.map(|b| b.join("config.toml"));
        let mut level = "info".to_string();

        if let Some(path) = config_path {
            if let Ok(c) = NodeConfig::load(&path) {
                level = c.log_level;
            }
        }
        level
    };

    // If RUST_LOG is already set in the environment, use it (developer override).
    // Otherwise, use the level from the config file, but add some noise reduction
    // for third-party crates like mdns_sd and quinn which are very verbose.
    let filter = if let Ok(env) = std::env::var("RUST_LOG") {
        env
    } else if log_level == "debug" {
        "debug,mdns_sd=info,quinn=info".to_string()
    } else if log_level == "trace" {
        "trace,mdns_sd=info,quinn=info".to_string()
    } else {
        log_level
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&filter))
        .try_init();

    let builder = tauri::Builder::default();
    #[cfg(mobile)]
    let builder = builder.plugin(mobile_keychain::init());
    #[cfg(not(mobile))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        // A second instance was launched — focus the existing window instead.
        if let Some(win) = app.get_webview_window("main") {
            info!("Second instance launched, focusing existing window");
            let _ = win.show();
            let _ = win.set_focus();
        }
    }));
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let handle = app.handle().clone();

            tauri::async_runtime::spawn(async move {
                check_backend_status(handle).await;
            });

            #[cfg(desktop)]
            setup_tray(app)?;
            Ok(())
        })
        .on_window_event(|_window, _event| {
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = _event {
                api.prevent_close();
                let _ = _window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_backend_status,
            pick_file,
            unlock_identity,
            create_identity,
            get_save_passphrase,
            set_save_passphrase,
            commands::node_send_message,
            commands::node_get_messages,
            commands::node_send_dm,
        ])
        .run(tauri::generate_context!())
        .expect("error while running BitCord");
}
