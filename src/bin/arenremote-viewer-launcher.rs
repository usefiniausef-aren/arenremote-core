#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
mod app {
    use std::{
        env,
        error::Error,
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::Duration,
    };

    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use url::Url;
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_WRITE},
        RegKey,
    };
    use windows::{
        core::PCWSTR,
        Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
        },
    };

    const APP_TITLE: &str = "Aren Remote";
    const PROTOCOL: &str = "arenremote";
    const INSTALL_DIR_NAME: &str = "ArenRemoteViewer";
    const LAUNCHER_FILE: &str = "ArenRemote.ViewerLauncher.exe";
    const CORE_FILE: &str = "ArenRemote.Core.exe";
    const SCITER_FILE: &str = "sciter.dll";
    const VIEWER_RESOLVE_BASE_URL: &str = "https://crm.aren-co.ir/RemoteSupport/ResolveViewer/";
    const CUSTOMER_RESOLVE_BASE_URL: &str = "https://crm.aren-co.ir/RemoteSupport/ResolveCustomer/";
    const MAX_CUSTOMER_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ViewerResolveResponse {
        remote_device_id: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CustomerResolveResponse {
        package_url: String,
        package_sha256: String,
    }

    enum ProtocolAction {
        Technician(String),
        Customer(String),
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        let args: Vec<String> = env::args().collect();

        match args.get(1).map(String::as_str) {
            Some("--install") => {
                install()?;
                if !args.iter().any(|x| x == "--silent") {
                    show_info("Aren Remote با موفقیت برای این کاربر فعال شد. از این پس پشتیبانی مشتری و اتصال کارشناس از داخل ArenCRM انجام می‌شود.");
                }
                Ok(())
            }
            Some("--uninstall") => {
                uninstall()?;
                show_info("Aren Remote برای این کاربر غیرفعال شد.");
                Ok(())
            }
            Some("--connect") => {
                let id = args.get(2).ok_or("Remote ID is required")?;
                connect(id)
            }
            Some(value) if value.to_ascii_lowercase().starts_with("arenremote://") => {
                match parse_protocol_uri(value).ok_or("Invalid Aren Remote protocol URL")? {
                    ProtocolAction::Technician(token) => {
                        let remote_id = resolve_viewer_launch_token(&token)?;
                        connect(&remote_id)
                    }
                    ProtocolAction::Customer(token) => launch_customer_support(&token),
                }
            }
            Some(_) => {
                show_error("دستور Aren Remote معتبر نیست.");
                Ok(())
            }
            None => {
                install()?;
                show_info("Aren Remote آماده است. از این پس فقط از صفحه پشتیبانی ArenCRM استفاده کنید.");
                Ok(())
            }
        }
    }

    pub fn show_fatal(message: &str) {
        show_error(&format!(
            "راه‌اندازی Aren Remote انجام نشد.\n\n{message}\n\nبه صفحه پشتیبانی ArenCRM برگردید و دوباره تلاش کنید."
        ));
    }

    fn install() -> Result<(), Box<dyn Error>> {
        let source_launcher = env::current_exe()?;
        let source_dir = source_launcher
            .parent()
            .ok_or("Could not resolve Aren Remote source directory")?;

        let source_core = source_dir.join(CORE_FILE);
        let source_sciter = source_dir.join(SCITER_FILE);
        require_file(&source_core, CORE_FILE)?;
        require_file(&source_sciter, SCITER_FILE)?;

        let install_dir = install_dir()?;
        fs::create_dir_all(&install_dir)?;

        let installed_launcher = install_dir.join(LAUNCHER_FILE);
        let installed_core = install_dir.join(CORE_FILE);
        let installed_sciter = install_dir.join(SCITER_FILE);

        copy_if_different(&source_launcher, &installed_launcher)?;
        copy_if_different(&source_core, &installed_core)?;
        copy_if_different(&source_sciter, &installed_sciter)?;

        register_protocol(&installed_launcher)?;
        Ok(())
    }

    fn uninstall() -> Result<(), Box<dyn Error>> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let classes = hkcu.open_subkey_with_flags("Software\\Classes", KEY_WRITE)?;
        let _ = classes.delete_subkey_all(PROTOCOL);
        Ok(())
    }

    fn register_protocol(installed_launcher: &Path) -> Result<(), Box<dyn Error>> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (protocol_key, _) = hkcu.create_subkey(format!("Software\\Classes\\{PROTOCOL}"))?;
        protocol_key.set_value("", &"URL:Aren Remote Protocol")?;
        protocol_key.set_value("URL Protocol", &"")?;

        let (icon_key, _) = hkcu.create_subkey(format!("Software\\Classes\\{PROTOCOL}\\DefaultIcon"))?;
        icon_key.set_value("", &format!("\"{}\",0", installed_launcher.display()))?;

        let (command_key, _) = hkcu.create_subkey(format!(
            "Software\\Classes\\{PROTOCOL}\\shell\\open\\command"
        ))?;
        command_key.set_value(
            "",
            &format!("\"{}\" \"%1\"", installed_launcher.display()),
        )?;
        Ok(())
    }

    fn http_client() -> Result<reqwest::blocking::Client, Box<dyn Error>> {
        Ok(reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?)
    }

    fn resolve_viewer_launch_token(token: &str) -> Result<String, Box<dyn Error>> {
        validate_token(token)?;
        let url = format!("{VIEWER_RESOLVE_BASE_URL}{token}");
        let response = http_client()?.get(url).send()?.error_for_status()?;
        let payload: ViewerResolveResponse = response.json()?;
        normalize_remote_id(&payload.remote_device_id)
            .ok_or_else(|| "Invalid Remote ID returned by CRM".into())
    }

    fn launch_customer_support(token: &str) -> Result<(), Box<dyn Error>> {
        validate_token(token)?;
        let resolve_url = format!("{CUSTOMER_RESOLVE_BASE_URL}{token}");
        let client = http_client()?;
        let response = client.get(resolve_url).send()?.error_for_status()?;
        let payload: CustomerResolveResponse = response.json()?;

        let expected_sha = normalize_sha256(&payload.package_sha256)
            .ok_or("Invalid customer package SHA256 returned by CRM")?;
        validate_customer_package_url(&payload.package_url)?;

        let mut package_response = client
            .get(&payload.package_url)
            .send()?
            .error_for_status()?;

        if let Some(length) = package_response.content_length() {
            if length == 0 || length > MAX_CUSTOMER_PACKAGE_BYTES {
                return Err("Customer support package has an invalid size".into());
            }
        }

        let bytes = package_response.bytes()?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_CUSTOMER_PACKAGE_BYTES {
            return Err("Customer support package has an invalid size".into());
        }

        let actual_sha = hex::encode(Sha256::digest(bytes.as_ref()));
        if actual_sha != expected_sha {
            return Err("Customer support package SHA256 verification failed".into());
        }

        let install_dir = install_dir()?;
        let session_dir = install_dir.join("Sessions");
        fs::create_dir_all(&session_dir)?;
        cleanup_old_session_packages(&session_dir);

        let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
        let stem = &token_hash[..16];
        let temp_path = session_dir.join(format!("ArenRemote-{stem}.tmp"));
        let package_path = session_dir.join(format!("ArenRemote-{stem}.exe"));

        fs::write(&temp_path, bytes.as_ref())?;
        if package_path.exists() {
            let _ = fs::remove_file(&package_path);
        }
        fs::rename(&temp_path, &package_path)?;

        Command::new(&package_path)
            .current_dir(&session_dir)
            .spawn()?;

        Ok(())
    }

    fn validate_customer_package_url(raw: &str) -> Result<(), Box<dyn Error>> {
        let url = Url::parse(raw)?;
        if url.scheme() != "https"
            || url.host_str() != Some("crm.aren-co.ir")
            || url.port_or_known_default() != Some(443)
            || !url.path().starts_with("/RemoteSupport/")
        {
            return Err("CRM returned an untrusted customer package URL".into());
        }
        Ok(())
    }

    fn connect(raw_id: &str) -> Result<(), Box<dyn Error>> {
        let remote_id = normalize_remote_id(raw_id).ok_or("Invalid Remote ID")?;
        let install_dir = install_dir()?;
        let core = install_dir.join(CORE_FILE);
        let sciter = install_dir.join(SCITER_FILE);

        if !core.is_file() || !sciter.is_file() {
            show_error("Aren Remote کامل فعال نشده است. بسته Aren Remote را دوباره اجرا کنید.");
            return Ok(());
        }

        Command::new(&core)
            .arg("--connect")
            .arg(&remote_id)
            .current_dir(&install_dir)
            .spawn()?;

        Ok(())
    }

    fn parse_protocol_uri(uri: &str) -> Option<ProtocolAction> {
        let lower = uri.to_ascii_lowercase();
        for (prefix, is_customer) in [
            ("arenremote://launch/", false),
            ("arenremote://support/", true),
        ] {
            if lower.starts_with(prefix) {
                let token = &uri[prefix.len()..];
                let token = token
                    .split(['?', '#'])
                    .next()
                    .unwrap_or_default()
                    .trim_matches('/');
                if token.is_empty() {
                    return None;
                }
                return Some(if is_customer {
                    ProtocolAction::Customer(token.to_owned())
                } else {
                    ProtocolAction::Technician(token.to_owned())
                });
            }
        }
        None
    }

    fn validate_token(token: &str) -> Result<(), Box<dyn Error>> {
        if token.len() < 20
            || token.len() > 4096
            || !token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err("Invalid Aren Remote launch token".into());
        }
        Ok(())
    }

    fn normalize_remote_id(value: &str) -> Option<String> {
        let value = value.trim().trim_matches('/');
        if value.len() < 5 || value.len() > 32 {
            return None;
        }
        if !value.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Some(value.to_owned())
    }

    fn normalize_sha256(value: &str) -> Option<String> {
        let value = value.trim().to_ascii_lowercase();
        if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(value)
    }

    fn cleanup_old_session_packages(session_dir: &Path) {
        let cutoff = std::time::SystemTime::now()
            .checked_sub(Duration::from_secs(24 * 60 * 60));
        let Some(cutoff) = cutoff else {
            return;
        };
        let Ok(entries) = fs::read_dir(session_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let remove = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|modified| modified < cutoff)
                .unwrap_or(false);
            if remove {
                let _ = fs::remove_file(path);
            }
        }
    }

    fn install_dir() -> Result<PathBuf, Box<dyn Error>> {
        let local_app_data = env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?;
        Ok(PathBuf::from(local_app_data).join(INSTALL_DIR_NAME))
    }

    fn require_file(path: &Path, label: &str) -> Result<(), Box<dyn Error>> {
        if !path.is_file() {
            return Err(format!("Required Aren Remote file is missing: {label}").into());
        }
        Ok(())
    }

    fn copy_if_different(source: &Path, target: &Path) -> Result<(), Box<dyn Error>> {
        if source == target {
            return Ok(());
        }
        fs::copy(source, target)?;
        Ok(())
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn show_info(message: &str) {
        show_message(message, false);
    }

    fn show_error(message: &str) {
        show_message(message, true);
    }

    fn show_message(message: &str, is_error: bool) {
        let text = wide(message);
        let title = wide(APP_TITLE);
        unsafe {
            let style = if is_error {
                MB_OK | MB_ICONERROR
            } else {
                MB_OK | MB_ICONINFORMATION
            };
            let _ = MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                style,
            );
        }
    }
}

#[cfg(target_os = "windows")]
fn main() {
    if let Err(err) = app::run() {
        app::show_fatal(&err.to_string());
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Aren Remote Launcher is supported on Windows only.");
}
