use std::{
    env,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use commands::AppState;
use feishu_client::FeishuClient;
use tauri::Manager;

mod commands;
mod db;
mod sync;

fn main() {
    let env_path = load_env_file();

    let configured_db_path = env::var("FLASHIDEA_DB_PATH").ok();
    let db_path = resolve_db_path(configured_db_path.as_deref(), env_path.as_deref())
        .expect("failed to resolve sqlite database path");
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).expect("failed to create sqlite database directory");
    }
    let db_path = db_path.to_string_lossy().into_owned();
    let app_id = env::var("FEISHU_APP_ID").unwrap_or_default();
    let app_secret = env::var("FEISHU_APP_SECRET").unwrap_or_default();
    let doc_id = env::var("FEISHU_DOC_ID").unwrap_or_default();
    let wiki_node_token = env::var("FEISHU_WIKI_NODE_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty());

    eprintln!("FEISHU_APP_ID set: {}", !app_id.trim().is_empty());
    eprintln!(
        "FEISHU_WIKI_NODE_TOKEN set: {}",
        wiki_node_token.is_some()
    );

    let conn = db::init_db(&db_path).expect("failed to initialize sqlite database");
    let feishu_client = Arc::new(FeishuClient::new(app_id, app_secret));

    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        feishu_client: feishu_client.clone(),
        doc_id: doc_id.clone(),
        wiki: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .manage(state)
        .setup(move |app| {
            let state = app.state::<AppState>();
            let app_handle = app.handle().clone();
            let db = Arc::clone(&state.db);
            let client = Arc::clone(&state.feishu_client);
            let wiki_holder = Arc::clone(&state.wiki);
            let doc_id = state.doc_id.clone();
            let node_token = wiki_node_token.clone();

            tauri::async_runtime::spawn(async move {
                let wiki = if let Some(ref token) = node_token {
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

                if let Some(ref w) = wiki {
                    if let Ok(mut guard) = wiki_holder.lock() {
                        *guard = Some(Arc::clone(w));
                    }
                }

                sync::sync_all_queued(client, db, wiki, doc_id, app_handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::send_message,
            commands::get_messages,
            commands::retry_message
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
        .unwrap_or_else(|| PathBuf::from("flashidea.sqlite"));

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
            "flashidea-main-test-{name}-{}",
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
        assert_eq!(db_path, root.join("flashidea.sqlite"));
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn relative_configured_db_path_is_resolved_from_workspace_root() {
        let root = temp_dir("relative-db-path");
        let env_path = root.join(".env");
        fs::write(&env_path, "FLASHIDEA_DB_PATH=data/flashidea.sqlite\n").expect("write env file");

        let db_path = resolve_db_path(Some("data/flashidea.sqlite"), Some(env_path.as_path()))
            .expect("resolve db path");

        assert_eq!(db_path, root.join("data").join("flashidea.sqlite"));
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn absolute_configured_db_path_is_preserved() {
        let configured = Path::new("/tmp/flashidea.sqlite");

        let db_path = resolve_db_path(configured.to_str(), None).expect("resolve db path");

        assert_eq!(db_path, configured);
    }
}
