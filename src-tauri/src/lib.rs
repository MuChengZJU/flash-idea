use std::{
    env,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use commands::AppState;
use feishu_client::FeishuClient;
use rusqlite::Connection;
use tauri::Manager;
use tokio::sync::RwLock;

mod commands;
mod db;
mod sync;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let env_path = load_env_file();

    tauri::Builder::default()
        .setup(move |app| {
            let db_path = resolve_db_path_for_app(app, env_path.as_deref())?;
            let db_path_str = db_path.to_string_lossy().into_owned();
            let doc_id = env::var("FEISHU_DOC_ID").unwrap_or_default();
            let conn = db::init_db(&db_path_str)
                .map_err(|e| format!("failed to initialize sqlite database: {e}"))?;
            let (app_id, app_secret, wiki_node_token) = load_credentials(&conn);

            eprintln!(
                "FEISHU_APP_ID prefix: {}",
                app_id.chars().take(6).collect::<String>()
            );
            eprintln!(
                "FEISHU_WIKI_NODE_TOKEN set: {}",
                wiki_node_token.is_some()
            );

            let feishu_client = Arc::new(FeishuClient::new(app_id, app_secret));

            let state = AppState {
                db: Arc::new(Mutex::new(conn)),
                feishu_client: Arc::new(RwLock::new(feishu_client)),
                doc_id: Arc::new(RwLock::new(doc_id)),
                wiki: Arc::new(RwLock::new(None)),
            };
            app.manage(state);

            let state = app.state::<AppState>();
            let app_handle = app.handle().clone();
            let db = Arc::clone(&state.db);
            let client_holder = Arc::clone(&state.feishu_client);
            let wiki_holder = Arc::clone(&state.wiki);
            let doc_id = Arc::clone(&state.doc_id);

            tauri::async_runtime::spawn(async move {
                let client = {
                    let guard = client_holder.read().await;
                    Arc::clone(&*guard)
                };

                let wiki = if let Some(ref token) = wiki_node_token {
                    match sync::init_wiki(&client, token).await {
                        Ok(cfg) => {
                            eprintln!("wiki init succeeded");
                            Some(Arc::new(cfg))
                        }
                        Err(e) => {
                            eprintln!("wiki init failed: {e}, falling back to single doc");
                            None
                        }
                    }
                } else {
                    eprintln!("wiki init skipped: FEISHU_WIKI_NODE_TOKEN is not set");
                    None
                };

                {
                    let mut guard = wiki_holder.write().await;
                    *guard = wiki;
                }

                sync::pull_remote_messages(
                    Arc::clone(&client_holder),
                    Arc::clone(&db),
                    Arc::clone(&wiki_holder),
                    app_handle.clone(),
                )
                .await;

                sync::sync_all_queued(client_holder, db, wiki_holder, doc_id, app_handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::send_message,
            commands::get_messages,
            commands::retry_message,
            commands::get_config,
            commands::save_config,
            commands::test_connection
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn resolve_db_path_for_app(
    app: &tauri::App,
    env_path: Option<&Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let configured_db_path = env::var("FLASH_IDEA_DB_PATH")
        .or_else(|_| env::var("FLASHIDEA_DB_PATH"))
        .ok();

    #[cfg(mobile)]
    let db_path = {
        let data_dir = app.path().app_data_dir()?;
        fs::create_dir_all(&data_dir)?;
        configured_db_path
            .filter(|s| !s.trim().is_empty())
            .map(|s| data_dir.join(s))
            .unwrap_or_else(|| {
                let legacy = data_dir.join("flashidea.sqlite");
                if legacy.exists() { legacy } else { data_dir.join("flash-idea.sqlite") }
            })
    };

    #[cfg(not(mobile))]
    let db_path = {
        let _ = app;
        let path = resolve_db_path(configured_db_path.as_deref(), env_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        path
    };

    Ok(db_path)
}

fn load_credentials(conn: &Connection) -> (String, String, Option<String>) {
    let app_id = env::var("FEISHU_APP_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| db::get_setting(conn, "feishu_app_id").ok().flatten())
        .unwrap_or_default();
    let app_secret = env::var("FEISHU_APP_SECRET")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| db::get_setting(conn, "feishu_app_secret").ok().flatten())
        .unwrap_or_default();
    let wiki_token = env::var("FEISHU_WIKI_NODE_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| db::get_setting(conn, "feishu_wiki_node_token").ok().flatten());

    (app_id, app_secret, wiki_token)
}

fn load_env_file() -> Option<PathBuf> {
    let env_path = find_dotenv_path();
    if let Some(ref path) = env_path {
        match dotenvy::from_path(path) {
            Ok(_) => eprintln!("loaded .env from {}", path.display()),
            Err(e) => eprintln!("failed to load .env from {}: {e}", path.display()),
        }
    } else {
        eprintln!("no .env file found while walking up from the current directory");
    }
    env_path
}

fn find_dotenv_path() -> Option<PathBuf> {
    if let Ok(current_dir) = env::current_dir() {
        if let Some(path) = find_file_upwards(&current_dir, ".env") {
            return Some(path);
        }
    }

    if let Some(path) = find_file_upwards(Path::new(env!("CARGO_MANIFEST_DIR")), ".env") {
        return Some(path);
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            return find_file_upwards(exe_dir, ".env");
        }
    }

    None
}

fn find_file_upwards(start: &Path, file_name: &str) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(file_name);
        if candidate.is_file() {
            return Some(candidate);
        }

        if !dir.pop() {
            return None;
        }
    }
}

fn resolve_db_path(
    configured_path: Option<&str>,
    env_path: Option<&Path>,
) -> std::io::Result<PathBuf> {
    let base_dir = match env_path.and_then(Path::parent) {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => env::current_dir()?.join(path),
        None => env::current_dir()?,
    };

    let db_path = configured_path
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("flash-idea.sqlite"));

    if db_path.is_absolute() {
        Ok(db_path)
    } else {
        Ok(base_dir.join(db_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "flash-idea-main-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn finds_dotenv_in_parent_directory() {
        let root = temp_dir("dotenv-parent");
        let nested = root.join("src-tauri").join("target");
        fs::create_dir_all(&nested).expect("create nested dir");
        fs::write(root.join(".env"), "FEISHU_APP_ID=test\n").expect("write env file");

        let found = find_file_upwards(&nested, ".env").expect("find env file");

        assert_eq!(found, root.join(".env"));
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn default_db_path_is_absolute_under_workspace_root() {
        let root = temp_dir("db-path");
        let env_path = root.join(".env");
        fs::write(&env_path, "FEISHU_APP_ID=test\n").expect("write env file");

        let db_path = resolve_db_path(None, Some(env_path.as_path())).expect("resolve db path");

        assert!(db_path.is_absolute());
        assert_eq!(db_path, root.join("flash-idea.sqlite"));
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn relative_configured_db_path_is_resolved_from_workspace_root() {
        let root = temp_dir("relative-db-path");
        let env_path = root.join(".env");
        fs::write(&env_path, "FLASH_IDEA_DB_PATH=data/flash-idea.sqlite\n").expect("write env file");

        let db_path = resolve_db_path(Some("data/flash-idea.sqlite"), Some(env_path.as_path()))
            .expect("resolve db path");

        assert_eq!(db_path, root.join("data").join("flash-idea.sqlite"));
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn absolute_configured_db_path_is_preserved() {
        let configured = Path::new("/tmp/flash-idea.sqlite");

        let db_path = resolve_db_path(configured.to_str(), None).expect("resolve db path");

        assert_eq!(db_path, configured);
    }
}
