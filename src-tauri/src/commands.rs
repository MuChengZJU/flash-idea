use std::sync::{Arc, Mutex};

use chrono::Utc;
use feishu_client::FeishuClient;
use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::{
    db::{self, Message},
    sync,
};

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub feishu_client: Arc<FeishuClient>,
    pub doc_id: String,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: String,
    pub status: String,
}

#[tauri::command]
pub async fn send_message(
    text: String,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<MessageResponse, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(MessageResponse {
            id: String::new(),
            status: "rejected".to_string(),
        });
    }

    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let doc_id = state.doc_id.clone();

    {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        db::insert_message(&conn, &id, &text, &created_at, Some(&doc_id))
            .map_err(|err| err.to_string())?;
    }

    let message = Message {
        id: id.clone(),
        text,
        created_at,
        sync_status: "queued".to_string(),
        retry_count: 0,
        target_doc_id: Some(doc_id.clone()),
        metadata: "{}".to_string(),
        synced_at: None,
    };

    tauri::async_runtime::spawn(sync::sync_message(
        Arc::clone(&state.feishu_client),
        Arc::clone(&state.db),
        doc_id,
        message,
        app_handle,
    ));

    Ok(MessageResponse {
        id,
        status: "queued".to_string(),
    })
}

#[tauri::command]
pub async fn get_messages(
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<Message>, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    db::get_messages(&conn, limit.unwrap_or(50)).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn retry_message(
    id: String,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let message = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        db::reset_for_retry(&conn, &id)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("message not found: {id}"))?
    };

    tauri::async_runtime::spawn(sync::sync_message(
        Arc::clone(&state.feishu_client),
        Arc::clone(&state.db),
        state.doc_id.clone(),
        message,
        app_handle,
    ));

    Ok(())
}
