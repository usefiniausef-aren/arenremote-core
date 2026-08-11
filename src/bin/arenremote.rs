#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use librustdesk::*;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const ARENREMOTE_APP_NAME: &str = "ArenRemote";
const ARENREMOTE_ID_SERVER: &str = "185.132.80.165";
const ARENREMOTE_PUBLIC_KEY: &str = "hrAl0y2Ydsr9yRVstP2gr8nevXv9RCcTlDRQYAFIn6o=";
const ARENCRM_PACKAGE_MAGIC: &[u8; 16] = b"ARENREMOTEV2PKG!";
const ARENCRM_PACKAGE_FOOTER_LEN: u64 = 36;
const ARENCRM_PACKAGE_VERSION: u32 = 2;
const ARENCRM_MAX_CONFIG_LEN: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
struct ArenCrmSessionConfig {
    session_id: String,
    pairing_token: String,
    hello_url: String,
    heartbeat_url: String,
    access_code: String,
    heartbeat_seconds: u64,
}

/// Establish Aren Remote's runtime identity before RustDesk's lazy configuration,
/// IPC paths, and rendezvous selection are initialized.
///
/// RustDesk remains the transport engine, while Aren Remote uses an isolated
/// application namespace and a consent-first support profile.
fn prepare_arenremote_runtime() {
    {
        let mut app_name = hbb_common::config::APP_NAME
            .write()
            .expect("ArenRemote APP_NAME lock poisoned");
        *app_name = ARENREMOTE_APP_NAME.to_owned();
    }

    // Fail closed onto Aren infrastructure rather than falling back to the
    // upstream public rendezvous network.
    {
        let mut prod_server = hbb_common::config::PROD_RENDEZVOUS_SERVER
            .write()
            .expect("ArenRemote rendezvous lock poisoned");
        *prod_server = ARENREMOTE_ID_SERVER.to_owned();
    }

    let options = [
        ("custom-rendezvous-server", ARENREMOTE_ID_SERVER),
        ("key", ARENREMOTE_PUBLIC_KEY),
        // Customer consent is mandatory: an incoming control request must be
        // approved locally rather than accepted by a reusable password.
        ("approve-mode", "click"),
        ("verification-method", "use-temporary-password"),
        // Phase-1 support profile: remote desktop control only. Extra channels
        // remain disabled until explicitly introduced and tested in ArenCRM.
        ("enable-keyboard", "Y"),
        ("enable-clipboard", "N"),
        ("enable-file-transfer", "N"),
        ("enable-camera", "N"),
        ("enable-terminal", "N"),
        ("enable-remote-restart", "N"),
        ("enable-tunnel", "N"),
        ("enable-block-input", "N"),
        ("enable-privacy-mode", "N"),
        ("enable-lan-discovery", "N"),
        ("allow-remote-config-modification", "N"),
        ("direct-server", "N"),
        ("enable-audio", "N"),
    ];

    for (key, value) in options {
        hbb_common::config::Config::set_option(key.to_owned(), value.to_owned());
    }

    // Keep the tested portable custom-server parser available to child/elevated
    // processes. This contains only the public server address and public key.
    std::env::set_var(
        common::PORTABLE_APPNAME_RUNTIME_ENV_KEY,
        format!(
            "{ARENREMOTE_APP_NAME}-host={ARENREMOTE_ID_SERVER},key={ARENREMOTE_PUBLIC_KEY},relay={ARENREMOTE_ID_SERVER}"
        ),
    );
}

/// Headless integration point for ArenCRM.
///
/// Usage:
///   ArenRemote.Core.exe --aren-session-info <output-json-path>
///
/// The command never opens the main UI. It reads the current local ArenRemote
/// ID through the same IPC/config path used by the engine and writes a compact
/// JSON document that ArenCRM can poll after starting the customer agent.
fn try_handle_arencrm_bridge_command() -> bool {
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(String::as_str) != Some("--aren-session-info") {
        return false;
    }

    let Some(output_path) = args.get(2) else {
        return true;
    };

    let remote_id = ipc::get_id();
    let remote_id = remote_id.trim();
    let ready = !remote_id.is_empty();

    // The engine-issued ID and fixed server value contain no JSON control
    // characters, so keep this dependency-free and deterministic for the CRM
    // launcher/bridge.
    let payload = format!(
        "{{\n  \"ready\": {},\n  \"remoteId\": \"{}\",\n  \"server\": \"{}\",\n  \"app\": \"{}\"\n}}\n",
        if ready { "true" } else { "false" },
        remote_id,
        ARENREMOTE_ID_SERVER,
        ARENREMOTE_APP_NAME
    );

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let _ = std::fs::write(output_path, payload.as_bytes());
    true
}

fn parse_arencrm_session_config(package_path: &Path) -> Option<ArenCrmSessionConfig> {
    let mut file = File::open(package_path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len < ARENCRM_PACKAGE_FOOTER_LEN {
        return None;
    }

    file.seek(SeekFrom::End(-(ARENCRM_PACKAGE_FOOTER_LEN as i64)))
        .ok()?;
    let mut footer = [0u8; ARENCRM_PACKAGE_FOOTER_LEN as usize];
    file.read_exact(&mut footer).ok()?;

    if &footer[..16] != ARENCRM_PACKAGE_MAGIC {
        return None;
    }

    let version = u32::from_le_bytes(footer[16..20].try_into().ok()?);
    if version != ARENCRM_PACKAGE_VERSION {
        return None;
    }

    let config_len = u64::from_le_bytes(footer[28..36].try_into().ok()?);
    if config_len == 0
        || config_len > ARENCRM_MAX_CONFIG_LEN
        || config_len + ARENCRM_PACKAGE_FOOTER_LEN > file_len
    {
        return None;
    }

    let config_offset = file_len - ARENCRM_PACKAGE_FOOTER_LEN - config_len;
    file.seek(SeekFrom::Start(config_offset)).ok()?;
    let mut config_bytes = vec![0u8; config_len as usize];
    file.read_exact(&mut config_bytes).ok()?;

    let value: serde_json::Value = serde_json::from_slice(&config_bytes).ok()?;
    let string_value = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };

    let session_id = string_value("sessionId");
    let pairing_token = string_value("pairingToken");
    let hello_url = string_value("helloUrl");
    let heartbeat_url = string_value("heartbeatUrl");
    if session_id.is_empty()
        || pairing_token.is_empty()
        || hello_url.is_empty()
        || heartbeat_url.is_empty()
    {
        return None;
    }

    let heartbeat_seconds = value
        .get("heartbeatSeconds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(5)
        .clamp(3, 30);

    Some(ArenCrmSessionConfig {
        session_id,
        pairing_token,
        hello_url,
        heartbeat_url,
        access_code: string_value("accessCode"),
        heartbeat_seconds,
    })
}

fn arencrm_package_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::<PathBuf>::new();
    if let Ok(path) = std::env::current_dir() {
        roots.push(path);
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        roots.push(PathBuf::from(user_profile).join("Downloads"));
    }

    let mut files = Vec::<(SystemTime, PathBuf)>::new();
    roots.sort();
    roots.dedup();

    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|x| x.to_str()) else {
                continue;
            };
            if !name.starts_with("ArenRemote-") || !name.ends_with(".exe") {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|x| x.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((modified, path));
        }
    }

    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().map(|(_, path)| path).collect()
}

fn find_arencrm_session_config() -> Option<ArenCrmSessionConfig> {
    for candidate in arencrm_package_candidates() {
        if let Some(config) = parse_arencrm_session_config(&candidate) {
            return Some(config);
        }
    }
    None
}

fn start_arencrm_quick_support_reporter(config: ArenCrmSessionConfig) {
    std::thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(12))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                log::warn!("ArenCRM session reporter HTTP client init failed: {err}");
                return;
            }
        };

        let mut remote_id = String::new();
        for _ in 0..120 {
            let current_id = ipc::get_id();
            let current_id = current_id.trim();
            if !current_id.is_empty() {
                remote_id = current_id.to_owned();
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }

        if remote_id.is_empty() {
            log::warn!(
                "ArenCRM quick-support session {} could not resolve the local ArenRemote ID",
                config.session_id
            );
            return;
        }

        let device_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows device".to_owned());
        let platform = format!("Windows {}", std::env::consts::ARCH);
        let client_version = "ArenRemote-R4";

        let hello_body = serde_json::json!({
            "sessionId": config.session_id.clone(),
            "pairingToken": config.pairing_token.clone(),
            "deviceName": device_name,
            "platform": platform,
            "clientVersion": client_version,
            "remoteDeviceId": remote_id.clone(),
        });

        let mut hello_registered = false;
        for _ in 0..30 {
            match client.post(&config.hello_url).json(&hello_body).send() {
                Ok(response) if response.status().is_success() => {
                    hello_registered = true;
                    log::info!(
                        "ArenCRM quick-support session {} registered Remote ID {}",
                        config.session_id,
                        remote_id
                    );
                    break;
                }
                Ok(response) => {
                    log::warn!(
                        "ArenCRM quick-support hello for session {} returned HTTP {}",
                        config.session_id,
                        response.status()
                    );
                }
                Err(err) => {
                    log::warn!(
                        "ArenCRM quick-support hello for session {} failed: {}",
                        config.session_id,
                        err
                    );
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }

        if !hello_registered {
            log::warn!(
                "ArenCRM quick-support session {} could not register Remote ID {}",
                config.session_id,
                remote_id
            );
        }

        let heartbeat_delay = Duration::from_secs(config.heartbeat_seconds);
        loop {
            std::thread::sleep(heartbeat_delay);

            let latest_id = ipc::get_id();
            let latest_id = latest_id.trim();
            if !latest_id.is_empty() && latest_id != remote_id.as_str() {
                remote_id = latest_id.to_owned();
                log::info!(
                    "ArenCRM quick-support session {} Remote ID changed to {}",
                    config.session_id,
                    remote_id
                );
            }

            let heartbeat_body = serde_json::json!({
                "sessionId": config.session_id.clone(),
                "pairingToken": config.pairing_token.clone(),
                "remoteDeviceId": remote_id.clone(),
            });

            match client
                .post(&config.heartbeat_url)
                .json(&heartbeat_body)
                .send()
            {
                Ok(response) if response.status().is_success() => {}
                Ok(response) => {
                    log::warn!(
                        "ArenCRM quick-support heartbeat for session {} returned HTTP {}",
                        config.session_id,
                        response.status()
                    );
                }
                Err(err) => {
                    log::warn!(
                        "ArenCRM quick-support heartbeat for session {} failed: {}",
                        config.session_id,
                        err
                    );
                }
            }
        }
    });
}

fn main() {
    prepare_arenremote_runtime();

    // ArenCRM bridge commands are intentionally handled before core_main() so
    // the short-lived helper process never opens the native UI or starts a
    // second customer server instance.
    if try_handle_arencrm_bridge_command() {
        return;
    }

    let is_quick_support = std::env::args().any(|arg| arg == "--quick_support");
    let arencrm_session = if is_quick_support {
        find_arencrm_session_config()
    } else {
        None
    };

    if let Some(args) = core_main::core_main().as_mut() {
        if let Some(config) = arencrm_session {
            log::info!(
                "ArenCRM quick-support integration active for session {} (code {})",
                config.session_id,
                config.access_code
            );
            start_arencrm_quick_support_reporter(config);
        }

        if args.is_empty() {
            // Customer agent mode: core_main() has already started the local
            // support server and portable service. Keep the process alive but
            // do not open the RustDesk/ArenRemote main window. ArenCRM will be
            // the customer-facing UI and incoming control still requires a
            // local approval prompt.
            loop {
                std::thread::park_timeout(std::time::Duration::from_secs(3600));
            }
        } else {
            // Explicit technician/session commands (for example --connect)
            // may still open the native remote-session window when ArenCRM
            // invokes them.
            ui::start(args);
        }
    }

    common::global_clean();
}
