use std::{
    env,
    sync::{Arc, Mutex},
};

use commands::AppState;
use feishu_client::FeishuClient;
use tauri::Manager;

mod commands;
mod db;
mod sync;

fn main() {
    dotenvy::dotenv().ok();

    let db_path = env::var("FLASHIDEA_DB_PATH").unwrap_or_else(|_| "flashidea.sqlite".to_string());
    let app_id = env::var("FEISHU_APP_ID").unwrap_or_default();
    let app_secret = env::var("FEISHU_APP_SECRET").unwrap_or_default();
    let doc_id = env::var("FEISHU_DOC_ID").unwrap_or_default();
    let wiki_node_token = env::var("FEISHU_WIKI_NODE_TOKEN").ok();

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
                        Ok(cfg) => Some(Arc::new(cfg)),
                        Err(e) => {
                            eprintln!("wiki init failed: {e}, falling back to single doc");
                            None
                        }
                    }
                } else {
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
