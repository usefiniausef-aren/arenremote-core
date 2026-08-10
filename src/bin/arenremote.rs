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

fn main() {
    prepare_arenremote_runtime();

    if let Some(args) = core_main::core_main().as_mut() {
        ui::start(args);
    }

    common::global_clean();
}
