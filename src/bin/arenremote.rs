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
/// This intentionally keeps the transport/core implementation compatible with
/// RustDesk while isolating Aren Remote from any separately installed RustDesk
/// instance on the same Windows machine.
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

    // Persist only Aren Remote's own non-secret connection settings. Because
    // APP_NAME is already ArenRemote, these are written to ArenRemote's config
    // namespace rather than RustDesk's namespace.
    hbb_common::config::Config::set_option(
        "custom-rendezvous-server".to_owned(),
        ARENREMOTE_ID_SERVER.to_owned(),
    );
    hbb_common::config::Config::set_option(
        "key".to_owned(),
        ARENREMOTE_PUBLIC_KEY.to_owned(),
    );

    // Keep the existing, tested portable custom-server parser available to all
    // child/elevated processes. The value contains only the public rendezvous
    // address and public verification key; no password or private key is embedded.
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
