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

    let conn = db::init_db(&db_path).expect("failed to initialize sqlite database");
    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        feishu_client: Arc::new(FeishuClient::new(app_id, app_secret)),
        doc_id,
    };

    tauri::Builder::default()
        .manage(state)
        .setup(|app| {
            let state = app.state::<AppState>();
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(sync::sync_all_queued(
                Arc::clone(&state.feishu_client),
                Arc::clone(&state.db),
                state.doc_id.clone(),
                app_handle,
            ));
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
