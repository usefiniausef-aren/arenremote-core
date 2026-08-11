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

    const APP_TITLE: &str = "Aren Remote Viewer";
    const PROTOCOL: &str = "arenremote";
    const INSTALL_DIR_NAME: &str = "ArenRemoteViewer";
    const LAUNCHER_FILE: &str = "ArenRemote.ViewerLauncher.exe";
    const CORE_FILE: &str = "ArenRemote.Core.exe";
    const SCITER_FILE: &str = "sciter.dll";
    const RESOLVE_BASE_URL: &str = "https://crm.aren-co.ir/RemoteSupport/ResolveViewer/";

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ResolveResponse {
        remote_device_id: String,
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        let args: Vec<String> = env::args().collect();

        match args.get(1).map(String::as_str) {
            Some("--install") => {
                install()?;
                show_info("Aren Remote Viewer با موفقیت برای این کاربر نصب و فعال شد.");
                Ok(())
            }
            Some("--uninstall") => {
                uninstall()?;
                show_info("Aren Remote Viewer برای این کاربر غیرفعال شد.");
                Ok(())
            }
            Some("--connect") => {
                let id = args.get(2).ok_or("Remote ID is required")?;
                connect(id)
            }
            Some(value) if value.to_ascii_lowercase().starts_with("arenremote://") => {
                let token = parse_protocol_uri(value).ok_or("Invalid Aren Remote protocol URL")?;
                let remote_id = resolve_launch_token(&token)?;
                connect(&remote_id)
            }
            Some(_) => {
                show_error("دستور Aren Remote Viewer معتبر نیست.");
                Ok(())
            }
            None => {
                install()?;
                show_info("Aren Remote Viewer آماده است. اکنون می‌توانید از داخل ArenCRM روی «بازکردن Viewer» کلیک کنید.");
                Ok(())
            }
        }
    }

    pub fn show_fatal(message: &str) {
        show_error(&format!("راه‌اندازی Viewer انجام نشد.\n\n{message}\n\nاگر صفحه نشست مدت زیادی باز بوده، به ArenCRM برگردید و دوباره Viewer را آماده کنید."));
    }

    fn install() -> Result<(), Box<dyn Error>> {
        let source_launcher = env::current_exe()?;
        let source_dir = source_launcher
            .parent()
            .ok_or("Could not resolve viewer source directory")?;

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

    fn resolve_launch_token(token: &str) -> Result<String, Box<dyn Error>> {
        if token.len() < 20
            || token.len() > 4096
            || !token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err("Invalid launch token".into());
        }

        let url = format!("{RESOLVE_BASE_URL}{token}");
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        let response = client.get(url).send()?.error_for_status()?;
        let payload: ResolveResponse = response.json()?;
        normalize_remote_id(&payload.remote_device_id).ok_or_else(|| "Invalid Remote ID returned by CRM".into())
    }

    fn connect(raw_id: &str) -> Result<(), Box<dyn Error>> {
        let remote_id = normalize_remote_id(raw_id).ok_or("Invalid Remote ID")?;
        let install_dir = install_dir()?;
        let core = install_dir.join(CORE_FILE);
        let sciter = install_dir.join(SCITER_FILE);

        if !core.is_file() || !sciter.is_file() {
            show_error("Aren Remote Viewer کامل نصب نشده است. بسته Viewer را دوباره اجرا کنید.");
            return Ok(());
        }

        Command::new(&core)
            .arg("--connect")
            .arg(&remote_id)
            .current_dir(&install_dir)
            .spawn()?;

        Ok(())
    }

    fn parse_protocol_uri(uri: &str) -> Option<String> {
        let lower = uri.to_ascii_lowercase();
        let prefix = "arenremote://launch/";
        if !lower.starts_with(prefix) {
            return None;
        }
        let token = &uri[prefix.len()..];
        let token = token.split(['?', '#']).next().unwrap_or_default().trim_matches('/');
        if token.is_empty() {
            return None;
        }
        Some(token.to_owned())
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

    fn install_dir() -> Result<PathBuf, Box<dyn Error>> {
        let local_app_data = env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?;
        Ok(PathBuf::from(local_app_data).join(INSTALL_DIR_NAME))
    }

    fn require_file(path: &Path, label: &str) -> Result<(), Box<dyn Error>> {
        if !path.is_file() {
            return Err(format!("Required viewer file is missing: {label}").into());
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
    eprintln!("Aren Remote Viewer Launcher is supported on Windows only.");
}
