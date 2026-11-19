#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[cfg(all(not(debug_assertions), not(feature = "custom-protocol")))]
compile_error!("Release builds require --features filesolutions/custom-protocol. Use scripts/build.ps1 to embed the frontend.");
use anyhow::{Context, Result};
mod window_chrome;
use filesolutions_core::{
    engine::Engine,
    model::{Config, Version},
};
use filesolutions_windows::{filesystem, integration, registration};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
struct AppState {
    engine: Arc<Engine>,
    intent: Mutex<Option<Vec<String>>>,
    update: tokio::sync::Mutex<Option<filesolutions_core::update::Update>>,
    downloaded: tokio::sync::Mutex<Option<(PathBuf, String, u64)>>,
    transfer_cancel: Arc<std::sync::atomic::AtomicBool>,
    transfer_lock: tokio::sync::Mutex<()>,
}
async fn window_on_main(app: tauri::AppHandle, payload: Value) -> Result<Value> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move ||{
        let result=(||->Result<Value>{
            let window=handle.get_webview_window("main").context("Window unavailable")?;
            let hwnd=windows::Win32::Foundation::HWND(window.hwnd()?.0);
            match payload["operation"].as_str().context("Missing window operation")? {
                "state"=>{},
                "minimize"=>window.minimize()?,
                "maximize"=>{if window.is_maximized()?{window.unmaximize()?;}else{window.maximize()?;}},
                "close"=>window.close()?,
                "systemMenu"=>window_chrome::system_menu(hwnd)?,
                "layout"=>{if window_chrome::available(hwnd){window_chrome::set_layout(hwnd,serde_json::from_value(payload["layout"].clone())?)?;}},
                _=>anyhow::bail!("Unknown window operation"),
            }
            Ok(json!({"maximized":window.is_maximized()?,"nativeFrame":!window_chrome::available(hwnd)}))
        })();
        let _=send.send(result);
    })?;
    receive.await?
}
fn show(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
#[tauri::command]
async fn command(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    action: String,
    payload: Option<Value>,
) -> std::result::Result<Value, String> {
    dispatch(app, &state, action, payload.unwrap_or(Value::Null))
        .await
        .map_err(|e| format!("{e:#}"))
}
async fn dispatch(
    app: tauri::AppHandle,
    state: &AppState,
    action: String,
    payload: Value,
) -> Result<Value> {
    let engine = &state.engine;
    let text = |key: &str| {
        payload[key]
            .as_str()
            .map(String::from)
            .context(format!("Missing {key}"))
    };
    match action.as_str() {
        "browse" => {
            let engine = engine.clone();
            let path = text("path")?;
            tokio::task::spawn_blocking(move || engine.browse(&path))
                .await?
        }
        "browserLocations" => filesolutions_windows::browser_assets::locations(),
        "shellIcons" => {
            let paths: Vec<String> = serde_json::from_value(payload["paths"].clone())?;
            tokio::task::spawn_blocking(move || filesolutions_windows::browser_assets::icons(&paths)).await?
        }
        "openNetwork" => shell_on_main(app, vec![], "network".into()).await,
        "backupSelectionPreview" => {
            let source = payload["path"].as_str().map(PathBuf::from);
            let destination = payload["destinationId"].as_str().map(String::from);
            let new_destination = if payload["newDestination"].is_object() {
                Some((payload["newDestination"]["name"].as_str().context("Missing destination name")?.to_owned(), payload["newDestination"]["path"].as_str().context("Missing destination path")?.to_owned()))
            } else { None };
            let engine = engine.clone();
            Ok(serde_json::to_value(tokio::task::spawn_blocking(move || engine.backup_selection_preview(source.as_deref(), destination.as_deref(), new_destination)).await??)?)
        }
        "browserState" => engine.browser_state(),
        "saveBrowserState" => {
            engine.save_browser_state(&payload)?;
            Ok(Value::Null)
        }
        "window" => window_on_main(app, payload).await,
        "shell" => {
            let paths: Vec<String> = serde_json::from_value(payload["paths"].clone())?;
            shell_on_main(app, paths, text("verb")?).await
        }
        "cancelTransfer" => {
            state
                .transfer_cancel
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Value::Null)
        }
        "transfer" => {
            let _guard = state
                .transfer_lock
                .try_lock()
                .context("A transfer is already running")?;
            let sources: Vec<String> = serde_json::from_value(payload["paths"].clone())?;
            let destination = PathBuf::from(text("destination")?);
            for source in sources
                .iter()
                .map(PathBuf::from)
                .chain(std::iter::once(destination.clone()))
            {
                anyhow::ensure!(
                    !filesystem::contains(&engine.state, &source)
                        && !filesystem::contains(&source, &engine.state),
                    "Application data cannot be transferred"
                );
            }
            let moving = payload["moving"].as_bool().unwrap_or(false);
            let cancel = state.transfer_cancel.clone();
            cancel.store(false, std::sync::atomic::Ordering::SeqCst);
            Ok(json!(
                tokio::task::spawn_blocking(move || filesolutions_core::operations::transfer(
                    &sources,
                    &destination,
                    moving,
                    &cancel
                ))
                .await??
            ))
        }
        "newFolder" | "rename" => {
            let path = PathBuf::from(text("path")?);
            let name = text("name")?;
            let part = filesystem::safe_relative(&name)?;
            anyhow::ensure!(part.components().count() == 1, "Enter a single name");
            filesystem::no_reparse(&path)?;
            anyhow::ensure!(
                !filesystem::contains(&engine.state, &path)
                    && !filesystem::contains(&path, &engine.state),
                "Application data is protected"
            );
            let destination = if action == "rename" {
                path.parent().context("Cannot rename a drive")?.join(part)
            } else {
                path.join(part)
            };
            anyhow::ensure!(
                !destination.exists(),
                "An item with that name already exists"
            );
            if action == "rename" {
                filesystem::move_new(&path, &destination)?;
            } else {
                std::fs::create_dir(&destination)?;
            }
            Ok(json!(filesystem::display(&destination)))
        }
        "snapshot" => Ok(serde_json::to_value(engine.snapshot().await?)?),
        "items" => {
            let engine = engine.clone();
            Ok(serde_json::to_value(
                tokio::task::spawn_blocking(move || engine.display_items()).await??,
            )?)
        }
        "intent" => Ok(json!(state.intent.lock().unwrap().take())),
        "pickRoot" => {
            let file = payload["file"].as_bool().unwrap_or(false);
            let path = tokio::task::spawn_blocking(move || {
                let dialog =
                    rfd::FileDialog::new().set_title("Choose an FileSolutions backup source");
                if file {
                    dialog.pick_file()
                } else {
                    dialog.pick_folder()
                }
            })
            .await?;
            Ok(json!(path.map(|p| filesystem::display(&p))))
        }
        "preview" => {
            let config: Config = serde_json::from_value(payload["config"].clone())?;
            let description = text("description")?;
            let engine = engine.clone();
            Ok(serde_json::to_value(
                tokio::task::spawn_blocking(move || engine.preview(config, description)).await??,
            )?)
        }
        "inheritPreview" => {
            let path = PathBuf::from(text("path")?);
            let engine = engine.clone();
            Ok(serde_json::to_value(
                tokio::task::spawn_blocking(move || engine.inherit_preview(&path)).await??,
            )?)
        }
        "shellPreview" => {
            let path = PathBuf::from(text("path")?);
            let include = payload["include"].as_bool().unwrap_or(false);
            let engine = engine.clone();
            Ok(serde_json::to_value(
                tokio::task::spawn_blocking(move || engine.shell_preview(&path, include)).await??,
            )?)
        }
        "apply" => {
            let before = engine.config()?;
            engine.apply_when_idle(text("token")?).await?;
            let after = engine.config()?;
            if before.start_at_sign_in != after.start_at_sign_in
                && std::env::var_os("FILESOLUTIONS_PROFILE").is_none()
            {
                integration::startup(after.start_at_sign_in, &std::env::current_exe()?)?;
            }
            Ok(Value::Null)
        }
        "undo" => {
            let engine = engine.clone();
            Ok(serde_json::to_value(
                tokio::task::spawn_blocking(move || engine.undo_preview()).await??,
            )?)
        }
        "pause" => {
            engine.pause(Some(payload["seconds"].as_i64().unwrap_or(0)))?;
            Ok(Value::Null)
        }
        "resume" => {
            engine.pause(None)?;
            Ok(Value::Null)
        }
        "syncNow" => {
            engine.sync_now()?;
            Ok(Value::Null)
        }
        "pickDestination" => {
            let path = tokio::task::spawn_blocking(|| rfd::FileDialog::new()
                .set_title("Choose a backup destination (local folder, synced folder or network share)").pick_folder()).await?;
            Ok(json!(path.map(|p| filesystem::display(&p))))
        }
        "checkUpdate" => {
            let available = filesolutions_core::update::check(env!("CARGO_PKG_VERSION")).await?;
            let mut downloaded = state.downloaded.lock().await;
            let mut saved = state.update.lock().await;
            if saved.as_ref().map(|u| &u.version) != available.as_ref().map(|u| &u.version) {
                *downloaded = None;
            }
            *saved = available.clone();
            if available.is_some() {
                show(&app);
            }
            Ok(serde_json::to_value(available)?)
        }
        "downloadUpdate" => {
            let mut downloaded = state.downloaded.lock().await;
            let update = state
                .update
                .lock()
                .await
                .clone()
                .context("Check for an update first")?;
            let path = filesolutions_core::update::download(&update, &engine.state).await?;
            let hash_path = path.clone();
            let (hash, _, size) = tokio::task::spawn_blocking(move || {
                filesolutions_core::staging::hashes(std::fs::File::open(hash_path)?)
            })
            .await??;
            *downloaded = Some((path, hash, size));
            Ok(Value::Null)
        }
        "installUpdate" => {
            let downloaded = state.downloaded.lock().await;
            let (path, hash, size) = downloaded
                .as_ref()
                .context("Download the update first")?
                .clone();
            tokio::task::spawn_blocking(move || {
                filesolutions_core::update::verify_download(&path, &hash, size)?;
                launch_update(&path)
            })
            .await??;
            engine.stop_when_idle().await;
            app.exit(0);
            Ok(Value::Null)
        }
        "openDestination" => {
            let config = engine.config()?;
            let path = filesolutions_core::folder::base(&config)?;
            std::process::Command::new("explorer.exe")
                .arg(path)
                .spawn()?;
            Ok(Value::Null)
        }
        "catalogs" => engine.catalogs().await,
        "readCatalog" => Ok(serde_json::to_value(
            engine.read_catalog(&text("id")?).await?,
        )?),
        "restore" => {
            let version: Version = serde_json::from_value(payload["version"].clone())?;
            let relative = text("relative")?;
            let destination = tokio::task::spawn_blocking(|| {
                rfd::FileDialog::new()
                    .set_title("Restore outside your backup folders")
                    .pick_folder()
            })
            .await?;
            if let Some(destination) = destination {
                let engine = engine.clone();
                Ok(json!(
                    tokio::task::spawn_blocking(move || engine.restore(
                        version,
                        relative,
                        destination
                    ))
                    .await??
                ))
            } else {
                Ok(Value::Null)
            }
        }
        "acknowledgeConflict" => {
            engine.acknowledge_conflict(&text("id")?)?;
            Ok(Value::Null)
        }
        "inventory" => Ok(serde_json::to_value(registration::inventory()?)?),
        "takeover" => {
            let selected: Vec<(String, String)> =
                serde_json::from_value(payload["selected"].clone())?;
            anyhow::ensure!(!selected.is_empty(), "Select specific registrations first");
            let path = engine.state.join("overlay-selection.json");
            std::fs::write(&path, serde_json::to_vec(&selected)?)?;
            let arg = format!("takeover-json \"{}\"", path.display());
            tokio::task::spawn_blocking(move || elevated(&arg)).await??;
            Ok(Value::Null)
        }
        "restoreOverlays" => {
            let receipt = tokio::task::spawn_blocking(|| {
                rfd::FileDialog::new()
                    .set_title("Choose FileSolutions overlay recovery receipt")
                    .add_filter("Overlay recovery receipt", &["json"])
                    .pick_file()
            })
            .await?;
            if let Some(path) = receipt {
                let arg = format!("restore \"{}\"", path.display());
                tokio::task::spawn_blocking(move || elevated(&arg)).await??;
            }
            Ok(Value::Null)
        }
        "registerShell" => {
            tokio::task::spawn_blocking(|| elevated("register --overlays --context")).await??;
            Ok(Value::Null)
        }
        "hide" => {
            if let Some(window) = app.get_webview_window("main") {
                window.hide()?;
            }
            Ok(Value::Null)
        }
        "quit" => {
            engine.shutdown();
            app.exit(0);
            Ok(Value::Null)
        }
        _ => anyhow::bail!("Unknown application command"),
    }
}
async fn shell_on_main(app: tauri::AppHandle, paths: Vec<String>, verb: String) -> Result<Value> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| -> Result<Value> {
            let hwnd = windows::Win32::Foundation::HWND(
                handle
                    .get_webview_window("main")
                    .context("Window unavailable")?
                    .hwnd()?
                    .0,
            );
            use filesolutions_windows::explorer;
            match verb.as_str() {
                "network" => filesolutions_windows::browser_assets::open_network(hwnd)?,
                "systemMenu" => explorer::system_menu(hwnd)?,
                "clipboard" => return explorer::clipboard(hwnd),
                "delete" => explorer::recycle(hwnd, &paths)?,
                "context" => explorer::context(hwnd, &paths, None)?,
                "dragCopy" => explorer::drag_copy(hwnd, &paths)?,
                "open" | "properties" | "copy" | "cut" => {
                    explorer::context(hwnd, &paths, Some(&verb))?
                }
                _ => anyhow::bail!("Unsupported shell command"),
            }
            Ok(Value::Null)
        })();
        let _ = send.send(result.map_err(|e| e.to_string()));
    })?;
    receive.await?.map_err(anyhow::Error::msg)
}
fn launch_update(path: &std::path::Path) -> Result<()> {
    use windows::{
        core::{w, PCWSTR},
        Win32::UI::{
            Shell::{ShellExecuteExW, SHELLEXECUTEINFOW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    };
    let file = filesolutions_windows::wide(path);
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: w!("/WAITFORAPP"),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).context(
            "Installation was canceled or setup could not start. FileSolutions is still running.",
        )?;
    }
    Ok(())
}
fn elevated(arguments: &str) -> Result<()> {
    use windows::{
        core::{w, PCWSTR},
        Win32::{
            Foundation::CloseHandle,
            System::Threading::{GetExitCodeProcess, WaitForSingleObject},
            UI::{
                Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
                WindowsAndMessaging::SW_HIDE,
            },
        },
    };
    let helper = std::env::current_exe()?.with_file_name("filesolutions-setup.exe");
    anyhow::ensure!(
        helper.is_file(),
        "Install FileSolutions to use Explorer setup. The helper must be next to the app."
    );
    let file = filesolutions_windows::wide(helper);
    let arguments = filesolutions_windows::wide(arguments);
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(arguments.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info)
            .context("Windows setup elevation was canceled or could not start")?;
        WaitForSingleObject(info.hProcess, u32::MAX);
        let mut code = 1;
        let result = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        result?;
        anyhow::ensure!(code==0,"Explorer setup stopped (code {code}). Review the selected registrations and recovery receipts in ProgramData\\FileSolutions\\OverlayBackups.");
    }
    Ok(())
}
fn main() {
    if let Err(error) = start() {
        rfd::MessageDialog::new()
            .set_title("FileSolutions could not start")
            .set_description(format!("{error:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}
fn start() -> Result<()> {
    let mut context = tauri::generate_context!();
    if let Some(profile) = std::env::var_os("FILESOLUTIONS_PROFILE") {
        let profile = PathBuf::from(profile);
        anyhow::ensure!(
            profile.is_absolute(),
            "Isolated profile must be an absolute path"
        );
        filesolutions_windows::security::private_directory(&profile, false)?;
        let profile = std::fs::canonicalize(profile)?;
        context.config_mut().identifier = isolated_identifier(&context.config().identifier, &profile);
        std::env::set_var("FILESOLUTIONS_PROFILE", &profile);
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", profile.join("webview"));
    }
    let initial: Vec<_> = std::env::args().collect();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            *state.intent.lock().unwrap() = Some(args);
            show(app);
            let _ = app.emit("shell-intent", ());
        }))
        .invoke_handler(tauri::generate_handler![command]);
    let app = builder
        .setup(move |app| {
            let window=app.get_webview_window("main").context("Window unavailable")?;
            let hwnd=windows::Win32::Foundation::HWND(window.hwnd()?.0);
            if let Err(error)=window_chrome::install(hwnd){
                // MessageSolutions' fail-safe: preserve a usable frame if subclass installation fails.
                window.set_decorations(true)?;
                eprintln!("Custom window controls unavailable; using Windows frame: {error:#}");
            }
            let isolated = std::env::var_os("FILESOLUTIONS_PROFILE").map(PathBuf::from);
            let directory = isolated.clone().unwrap_or(app.path().app_local_data_dir()?);
            let engine = Engine::open(directory)?;
            // Refresh the existing user's choice to this version's executable after upgrade.
            if isolated.is_none() {
                integration::startup(engine.config()?.start_at_sign_in, &std::env::current_exe()?)?;
            }
            app.manage(AppState {
                engine: engine.clone(),
                intent: Mutex::new(Some(initial.clone())),
                update: tokio::sync::Mutex::new(None),
                downloaded: tokio::sync::Mutex::new(None),
                transfer_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                transfer_lock: tokio::sync::Mutex::new(()),
            });
            let handle = app.handle().clone();
            let open = MenuItem::with_id(app, "open", "Open FileSolutions", true, None::<&str>)?;
            let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
            let pause = MenuItem::with_id(app, "pause", "Pause backup", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit FileSolutions", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &sync, &pause, &quit])?;
            let tray = TrayIconBuilder::with_id("filesolutions")
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../../assets/tray/idle.png"
                ))?)
                .tooltip("FileSolutions — starting")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    let state = app.state::<AppState>();
                    match event.id.as_ref() {
                        "open" => show(app),
                        "sync" => {
                            if let Err(error) = state.engine.sync_now() {
                                let _ = app.emit("app-notice", error.to_string());
                                show(app);
                            }
                        }
                        "pause" => {
                            let paused = state.engine.config().map(|c| c.paused()).unwrap_or(false);
                            let _ = state.engine.pause(if paused { None } else { Some(0) });
                        }
                        "quit" => {
                            state.engine.shutdown();
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        show(tray.app_handle());
                    }
                })
                .build(app)?;
            if !initial.iter().any(|a| a == "--tray") {
                show(&handle);
            }
            let runner = engine.clone();
            tauri::async_runtime::spawn(async move {
                runner.run().await;
            });
            tauri::async_runtime::spawn(async move {
                let frames = tray_frames();
                let mut frame = 0;
                loop {
                    if let Ok((paused, phase, counts)) = engine.tray_status() {
                        let _ = pause.set_text(if paused {
                            "Resume backup"
                        } else {
                            "Pause backup"
                        });
                        let _ = sync.set_enabled(!paused);
                        let active = phase == "syncing" && !paused;
                        let bytes = if active && !filesystem::reduced_motion() {
                            let bytes = frames[frame % frames.len()];
                            frame += 1;
                            bytes
                        } else if paused {
                            include_bytes!("../../assets/tray/paused.png").as_slice()
                        } else if phase == "issue" {
                            include_bytes!("../../assets/tray/issue.png").as_slice()
                        } else if phase == "disconnected" {
                            include_bytes!("../../assets/tray/offline.png").as_slice()
                        } else if counts.pending > 0 {
                            include_bytes!("../../assets/tray/pending.png").as_slice()
                        } else if counts.backed_up > 0 {
                            include_bytes!("../../assets/tray/backed.png").as_slice()
                        } else {
                            include_bytes!("../../assets/tray/idle.png").as_slice()
                        };
                        if let Ok(image) = tauri::image::Image::from_bytes(bytes) {
                            let _ = tray.set_icon(Some(image));
                        }
                        let _ = tray.set_tooltip(Some(format!(
                            "FileSolutions — {}\n{} backed up · {} pending · {} issues",
                            phase, counts.backed_up, counts.pending, counts.issues
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event,tauri::WindowEvent::Resized(_)|tauri::WindowEvent::ScaleFactorChanged{..}){
                let _=window.emit("window-state",json!({"maximized":window.is_maximized().unwrap_or(false),"nativeFrame":window.is_decorated().unwrap_or(false)}));
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(context)?;
    let mutex = if std::env::var_os("FILESOLUTIONS_PROFILE").is_some() { None } else { unsafe {
        windows::Win32::System::Threading::CreateMutexW(
            None,
            false,
            windows::core::w!("Global\\FileSolutions-App-Running"),
        )
        .ok()
    }};
    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app.state::<AppState>().engine.shutdown();
        }
    });
    if let Some(mutex) = mutex {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(mutex);
        }
    }
    Ok(())
}
fn isolated_identifier(base: &str, canonical_profile: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    filesystem::key(canonical_profile).hash(&mut hash);
    format!("{base}.fixture-{:016x}", hash.finish())
}
#[cfg(test)]
mod isolated_instance_tests {
    use super::*;
    #[test]
    fn profile_instances_cannot_forward_into_production_or_other_profiles() {
        let base = "app.filesolutions.desktop";
        let first = isolated_identifier(base, std::path::Path::new(r"C:\Fixture\One"));
        assert_ne!(first, base);
        assert_eq!(first, isolated_identifier(base, std::path::Path::new(r"c:\fixture\one")));
        assert_ne!(first, isolated_identifier(base, std::path::Path::new(r"C:\Fixture\Two")));
    }
}
include!("tray_frames.rs");
