#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use librustdesk::*;

const ARENREMOTE_APP_NAME: &str = "ArenRemote";
const ARENREMOTE_ID_SERVER: &str = "185.132.80.165";
const ARENREMOTE_PUBLIC_KEY: &str = "hrAl0y2Ydsr9yRVstP2gr8nevXv9RCcTlDRQYAFIn6o=";

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

    let remote_id = crate::ipc::get_id();
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

fn main() {
    prepare_arenremote_runtime();

    // ArenCRM bridge commands are intentionally handled before core_main() so
    // the short-lived helper process never opens the native UI or starts a
    // second customer server instance.
    if try_handle_arencrm_bridge_command() {
        return;
    }

    if let Some(args) = core_main::core_main().as_mut() {
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
