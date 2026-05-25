use std::{
    env,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use feishu_client::{FeishuClient, FeishuError};
use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, State};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    db::{self, Message},
    sync::{self, WikiConfig},
};

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub feishu_client: Arc<RwLock<Arc<FeishuClient>>>,
    pub doc_id: Arc<RwLock<String>>,
    pub wiki: Arc<RwLock<Option<Arc<WikiConfig>>>>,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub configured: bool,
    pub app_id: String,
    pub app_secret_hint: String,
    pub wiki_node_token: String,
    pub from_env: bool,
}

#[derive(Debug, Serialize)]
pub struct TestResult {
    pub success: bool,
    pub token_ok: bool,
    pub wiki_ok: bool,
    pub error: Option<String>,
}

const APP_ID_KEY: &str = "feishu_app_id";
const APP_SECRET_KEY: &str = "feishu_app_secret";
const WIKI_NODE_TOKEN_KEY: &str = "feishu_wiki_node_token";
const APP_ID_ENV: &str = "FEISHU_APP_ID";
const APP_SECRET_ENV: &str = "FEISHU_APP_SECRET";
const WIKI_NODE_TOKEN_ENV: &str = "FEISHU_WIKI_NODE_TOKEN";

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

    {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        db::insert_message(&conn, &id, &text, &created_at, None)
            .map_err(|err| err.to_string())?;
    }

    let message = Message {
        id: id.clone(),
        text,
        created_at,
        sync_status: "queued".to_string(),
        retry_count: 0,
        target_doc_id: None,
        metadata: "{}".to_string(),
        synced_at: None,
        error_reason: None,
    };

    tauri::async_runtime::spawn(sync::sync_message(
        Arc::clone(&state.feishu_client),
        Arc::clone(&state.db),
        Arc::clone(&state.wiki),
        Arc::clone(&state.doc_id),
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
        Arc::clone(&state.wiki),
        Arc::clone(&state.doc_id),
        message,
        app_handle,
    ));

    Ok(())
}

#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<ConfigResponse, String> {
    let (app_id, app_secret, wiki_node_token, from_env) = read_effective_config(&state)?;
    Ok(build_config_response(
        app_id,
        app_secret,
        wiki_node_token,
        from_env,
    ))
}

#[tauri::command]
pub async fn save_config(
    app_id: String,
    app_secret: String,
    wiki_node_token: String,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<ConfigResponse, String> {
    if config_from_env() {
        let (app_id, app_secret, wiki_node_token, from_env) = read_effective_config(&state)?;
        return Ok(build_config_response(
            app_id,
            app_secret,
            wiki_node_token,
            from_env,
        ));
    }

    let app_id = app_id.trim().to_string();
    let app_secret = app_secret.trim().to_string();
    let wiki_node_token = wiki_node_token.trim().to_string();

    if app_id.is_empty() {
        return Err("App ID 不能为空".to_string());
    }

    let effective_secret = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        if app_secret.is_empty() {
            db::get_setting(&conn, APP_SECRET_KEY)
                .map_err(|err| err.to_string())?
                .unwrap_or_default()
        } else {
            app_secret.clone()
        }
    };

    if effective_secret.trim().is_empty() {
        return Err("App Secret 不能为空".to_string());
    }

    {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        db::set_setting(&conn, APP_ID_KEY, &app_id).map_err(|err| err.to_string())?;
        if !app_secret.is_empty() {
            db::set_setting(&conn, APP_SECRET_KEY, &app_secret).map_err(|err| err.to_string())?;
        }
        db::set_setting(&conn, WIKI_NODE_TOKEN_KEY, &wiki_node_token)
            .map_err(|err| err.to_string())?;
    }

    eprintln!(
        "save_config: updating Feishu credentials app_id_prefix={}",
        app_id_prefix(&app_id)
    );

    let client = Arc::new(FeishuClient::new(app_id.clone(), effective_secret.clone()));
    {
        let mut guard = state.feishu_client.write().await;
        *guard = Arc::clone(&client);
    }

    let wiki = if wiki_node_token.is_empty() {
        None
    } else {
        match sync::init_wiki(&client, &wiki_node_token).await {
            Ok(cfg) => Some(Arc::new(cfg)),
            Err(err) => {
                eprintln!(
                    "save_config: wiki init failed for app_id_prefix={}: {}",
                    app_id_prefix(&app_id),
                    err
                );
                None
            }
        }
    };

    {
        let mut guard = state.wiki.write().await;
        *guard = wiki;
    }

    tauri::async_runtime::spawn(sync::sync_all_queued(
        Arc::clone(&state.feishu_client),
        Arc::clone(&state.db),
        Arc::clone(&state.wiki),
        Arc::clone(&state.doc_id),
        app_handle,
    ));

    Ok(build_config_response(
        app_id,
        effective_secret,
        wiki_node_token,
        false,
    ))
}

#[tauri::command]
pub async fn test_connection(
    app_id: Option<String>,
    app_secret: Option<String>,
    wiki_node_token: Option<String>,
    state: State<'_, AppState>,
) -> Result<TestResult, String> {
    let form_app_id = app_id.unwrap_or_default().trim().to_string();
    let form_app_secret = app_secret.unwrap_or_default().trim().to_string();
    let form_wiki_node_token = wiki_node_token.unwrap_or_default().trim().to_string();

    let (saved_app_id, saved_app_secret, saved_wiki_node_token, _) = read_effective_config(&state)?;
    let use_form_credentials = !form_app_id.is_empty() && !form_app_secret.is_empty();

    if !use_form_credentials && (saved_app_id.trim().is_empty() || saved_app_secret.trim().is_empty()) {
        return Ok(TestResult {
            success: false,
            token_ok: false,
            wiki_ok: false,
            error: Some("请先填写 App ID 和 App Secret".to_string()),
        });
    }

    let client = if use_form_credentials {
        Arc::new(FeishuClient::new(form_app_id, form_app_secret))
    } else {
        let guard = state.feishu_client.read().await;
        Arc::clone(&*guard)
    };

    let wiki_token = if form_wiki_node_token.is_empty() {
        saved_wiki_node_token
    } else {
        form_wiki_node_token
    };

    if !wiki_token.trim().is_empty() {
        return match sync::init_wiki(&client, &wiki_token).await {
            Ok(_) => {
                if let Ok(conn) = state.db.lock() {
                    let _ = db::insert_log(&conn, "info", "test", "连接测试通过，wiki 可用");
                }
                Ok(TestResult {
                    success: true,
                    token_ok: true,
                    wiki_ok: true,
                    error: None,
                })
            }
            Err(err) => {
                if let Ok(conn) = state.db.lock() {
                    let _ = db::insert_log(&conn, "error", "test", &format!("连接测试失败: {err}"));
                }
                Ok(test_error_result(err, true))
            }
        };
    }

    match client
        .append_text(
            "__flash_idea_connection_test__",
            "Flash Idea connection test",
            "flash-idea-connection-test",
        )
        .await
    {
        Ok(()) | Err(FeishuError::ApiError { .. }) => {
            if let Ok(conn) = state.db.lock() {
                let _ = db::insert_log(&conn, "info", "test", "连接测试通过（无 wiki 配置）");
            }
            Ok(TestResult {
                success: true,
                token_ok: true,
                wiki_ok: true,
                error: None,
            })
        }
        Err(err) => {
            if let Ok(conn) = state.db.lock() {
                let _ = db::insert_log(&conn, "error", "test", &format!("连接测试失败: {err}"));
            }
            Ok(test_error_result(err, false))
        }
    }
}

#[tauri::command]
pub async fn export_logs(state: State<'_, AppState>) -> Result<String, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let logs = db::get_logs(&conn, 200).map_err(|e| e.to_string())?;
    let failed = db::get_failed_messages(&conn, 20).map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str(&format!("=== Flash Idea 诊断日志 ===\n"));
    out.push_str(&format!("版本: {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("导出时间: {}\n", chrono::Local::now().to_rfc3339()));

    let (app_id, _, wiki_node_token, from_env) = read_effective_config_from_conn(&conn);
    let app_id_display = if app_id.is_empty() { "未配置".to_string() } else { app_id_prefix(&app_id) };
    let wiki_display = if wiki_node_token.is_empty() { "未配置".to_string() } else { wiki_token_preview(&wiki_node_token) };
    out.push_str(&format!(
        "配置来源: {} | App ID: {} | Wiki Token: {}\n\n",
        if from_env { "环境变量" } else { "应用内" },
        app_id_display,
        wiki_display,
    ));

    out.push_str("--- 最近日志 ---\n");
    if logs.is_empty() {
        out.push_str("（无日志）\n");
    }
    for log in &logs {
        out.push_str(&format!("[{}] [{}] [{}] {}\n", log.created_at, log.level, log.tag, log.message));
    }

    if !failed.is_empty() {
        out.push_str("\n--- 失败消息 ---\n");
        for msg in &failed {
            out.push_str(&format!(
                "[{}] \"{}\" → {}\n",
                msg.created_at,
                failed_message_preview(&msg.text),
                msg.error_reason.as_deref().unwrap_or("未知错误"),
            ));
        }
    }

    Ok(out)
}

fn read_effective_config_from_conn(conn: &Connection) -> (String, String, String, bool) {
    let app_id = env_setting(APP_ID_ENV)
        .or_else(|| db::get_setting(conn, APP_ID_KEY).ok().flatten())
        .unwrap_or_default();
    let app_secret = env_setting(APP_SECRET_ENV)
        .or_else(|| db::get_setting(conn, APP_SECRET_KEY).ok().flatten())
        .unwrap_or_default();
    let wiki_node_token = env_setting(WIKI_NODE_TOKEN_ENV)
        .or_else(|| db::get_setting(conn, WIKI_NODE_TOKEN_KEY).ok().flatten())
        .unwrap_or_default();
    (app_id, app_secret, wiki_node_token, config_from_env())
}

fn read_effective_config(state: &State<'_, AppState>) -> Result<(String, String, String, bool), String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let app_id = env_setting(APP_ID_ENV)
        .or_else(|| db::get_setting(&conn, APP_ID_KEY).ok().flatten())
        .unwrap_or_default();
    let app_secret = env_setting(APP_SECRET_ENV)
        .or_else(|| db::get_setting(&conn, APP_SECRET_KEY).ok().flatten())
        .unwrap_or_default();
    let wiki_node_token = env_setting(WIKI_NODE_TOKEN_ENV)
        .or_else(|| db::get_setting(&conn, WIKI_NODE_TOKEN_KEY).ok().flatten())
        .unwrap_or_default();

    Ok((app_id, app_secret, wiki_node_token, config_from_env()))
}

fn build_config_response(
    app_id: String,
    app_secret: String,
    wiki_node_token: String,
    from_env: bool,
) -> ConfigResponse {
    ConfigResponse {
        configured: !app_id.trim().is_empty() && !app_secret.trim().is_empty(),
        app_id,
        app_secret_hint: secret_hint(&app_secret),
        wiki_node_token,
        from_env,
    }
}

fn env_setting(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn config_from_env() -> bool {
    env_setting(APP_ID_ENV).is_some() && env_setting(APP_SECRET_ENV).is_some()
}

fn secret_hint(secret: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 4 {
        return "****".to_string();
    }
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("****{suffix}")
}

fn wiki_token_preview(token: &str) -> String {
    let prefix: String = token.chars().take(8).collect();
    format!("{prefix}...")
}

fn failed_message_preview(text: &str) -> String {
    let prefix: String = text.chars().take(2).collect();
    format!("{prefix}***")
}

fn app_id_prefix(app_id: &str) -> String {
    app_id.chars().take(6).collect()
}

fn test_error_result(err: FeishuError, wiki_attempted: bool) -> TestResult {
    match err {
        FeishuError::AuthError(msg) => TestResult {
            success: false,
            token_ok: false,
            wiki_ok: false,
            error: Some(msg),
        },
        FeishuError::NetworkError(msg) => TestResult {
            success: false,
            token_ok: false,
            wiki_ok: false,
            error: Some(msg),
        },
        FeishuError::RateLimited => TestResult {
            success: false,
            token_ok: true,
            wiki_ok: !wiki_attempted,
            error: Some("飞书接口限流，请稍后重试".to_string()),
        },
        FeishuError::ApiError { code, msg } => TestResult {
            success: false,
            token_ok: true,
            wiki_ok: false,
            error: Some(format!("飞书接口错误 {code}: {msg}")),
        },
    }
}
