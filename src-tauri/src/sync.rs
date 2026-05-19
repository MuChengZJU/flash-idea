use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Local, NaiveTime, Utc};
use feishu_client::{FeishuClient, FeishuError};
use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::time::sleep;

use crate::db::{self, Message};

const DAY_BOUNDARY: NaiveTime = match NaiveTime::from_hms_opt(6, 0, 0) {
    Some(t) => t,
    None => unreachable!(),
};

#[derive(Clone, Serialize)]
struct SyncStatusChanged {
    id: String,
    status: String,
}

#[derive(Clone)]
pub struct WikiConfig {
    pub node_token: String,
    pub space_id: String,
}

pub async fn init_wiki(
    feishu_client: &FeishuClient,
    node_token: &str,
) -> Result<WikiConfig, FeishuError> {
    let node = feishu_client.get_wiki_node(node_token).await?;
    Ok(WikiConfig {
        node_token: node_token.to_string(),
        space_id: node.space_id,
    })
}

fn needs_new_doc(last_synced_at: Option<&str>, now_created_at: &str) -> bool {
    let now = match DateTime::parse_from_rfc3339(now_created_at) {
        Ok(dt) => dt.with_timezone(&Local),
        Err(_) => return false,
    };

    let last = match last_synced_at {
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(dt) => dt.with_timezone(&Local),
            Err(_) => return true,
        },
        None => return true,
    };

    let now_day_date = if now.time() < DAY_BOUNDARY {
        now.date_naive() - chrono::Duration::days(1)
    } else {
        now.date_naive()
    };

    let last_day_date = if last.time() < DAY_BOUNDARY {
        last.date_naive() - chrono::Duration::days(1)
    } else {
        last.date_naive()
    };

    now_day_date > last_day_date
}

async fn resolve_doc_id(
    feishu_client: &FeishuClient,
    db: &Mutex<Connection>,
    wiki: &WikiConfig,
    message: &Message,
) -> Result<String, FeishuError> {
    let (active_doc, last_synced) = {
        let conn = db.lock().map_err(|e| FeishuError::ApiError {
            code: -1,
            msg: e.to_string(),
        })?;
        let doc = db::get_setting(&conn, "active_doc_id")
            .ok()
            .flatten();
        let last = db::get_last_synced_at(&conn).ok().flatten();
        (doc, last)
    };

    if let Some(ref doc_id) = active_doc {
        if !needs_new_doc(last_synced.as_deref(), &message.created_at) {
            return Ok(doc_id.clone());
        }
    }

    let now = DateTime::parse_from_rfc3339(&message.created_at)
        .unwrap_or_else(|_| Utc::now().fixed_offset())
        .with_timezone(&Local);

    let doc_date = if now.time() < DAY_BOUNDARY {
        now.date_naive() - chrono::Duration::days(1)
    } else {
        now.date_naive()
    };
    let title = format!("FlashIdea - {}", doc_date.format("%Y-%m-%d"));

    let node = feishu_client
        .create_wiki_child(&wiki.space_id, &wiki.node_token, &title)
        .await?;

    let new_doc_id = node.obj_token;
    if let Ok(conn) = db.lock() {
        let _ = db::set_setting(&conn, "active_doc_id", &new_doc_id);
    }

    Ok(new_doc_id)
}

pub async fn sync_message(
    feishu_client: Arc<FeishuClient>,
    db: Arc<Mutex<Connection>>,
    wiki: Option<Arc<WikiConfig>>,
    fallback_doc_id: String,
    message: Message,
    app_handle: AppHandle,
) {
    let doc_id = if let Some(ref wiki) = wiki {
        match resolve_doc_id(&feishu_client, &db, wiki, &message).await {
            Ok(id) => id,
            Err(_) => fallback_doc_id.clone(),
        }
    } else {
        fallback_doc_id.clone()
    };

    if let Ok(conn) = db.lock() {
        let _ = db::update_target_doc_id(&conn, &message.id, &doc_id);
    }

    let content = format_message_content(&message);
    let mut rate_limited_once = false;

    loop {
        match feishu_client
            .append_text(&doc_id, &content, &message.id)
            .await
        {
            Ok(()) => {
                let synced_at = Utc::now().to_rfc3339();
                if let Ok(conn) = db.lock() {
                    let _ = db::update_sync_status(&conn, &message.id, "synced", Some(&synced_at));
                }
                emit_status(&app_handle, &message.id, "synced");
                return;
            }
            Err(FeishuError::RateLimited) if !rate_limited_once => {
                rate_limited_once = true;
                sleep(Duration::from_millis(350)).await;
            }
            Err(FeishuError::NetworkError(_)) => {
                let retry_count = if let Ok(conn) = db.lock() {
                    db::increment_retry(&conn, &message.id).unwrap_or(message.retry_count + 1)
                } else {
                    message.retry_count + 1
                };

                if retry_count >= 5 {
                    if let Ok(conn) = db.lock() {
                        let _ = db::update_sync_status(&conn, &message.id, "failed", None);
                    }
                    emit_status(&app_handle, &message.id, "failed");
                }
                return;
            }
            Err(FeishuError::RateLimited)
            | Err(FeishuError::AuthError(_))
            | Err(FeishuError::ApiError { .. }) => {
                if let Ok(conn) = db.lock() {
                    let _ = db::update_sync_status(&conn, &message.id, "failed", None);
                }
                emit_status(&app_handle, &message.id, "failed");
                return;
            }
        }
    }
}

pub async fn sync_all_queued(
    feishu_client: Arc<FeishuClient>,
    db: Arc<Mutex<Connection>>,
    wiki: Option<Arc<WikiConfig>>,
    fallback_doc_id: String,
    app_handle: AppHandle,
) {
    let messages = if let Ok(conn) = db.lock() {
        db::get_queued_messages(&conn).unwrap_or_default()
    } else {
        Vec::new()
    };

    for message in messages {
        sync_message(
            Arc::clone(&feishu_client),
            Arc::clone(&db),
            wiki.clone(),
            fallback_doc_id.clone(),
            message,
            app_handle.clone(),
        )
        .await;
        sleep(Duration::from_millis(350)).await;
    }
}

fn format_message_content(message: &Message) -> String {
    let time = DateTime::parse_from_rfc3339(&message.created_at)
        .map(|dt| dt.with_timezone(&Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| message.created_at.clone());
    format!("[{}] {}", time, message.text)
}

fn emit_status(app_handle: &AppHandle, id: &str, status: &str) {
    let _ = app_handle.emit(
        "sync_status_changed",
        SyncStatusChanged {
            id: id.to_string(),
            status: status.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_new_doc_no_history() {
        assert!(needs_new_doc(None, "2026-05-19T10:00:00+08:00"));
    }

    #[test]
    fn test_needs_new_doc_same_day() {
        assert!(!needs_new_doc(
            Some("2026-05-19T10:00:00+08:00"),
            "2026-05-19T23:59:00+08:00"
        ));
    }

    #[test]
    fn test_needs_new_doc_late_night_same_day() {
        // 凌晨 2 点和凌晨 4 点属于同一个"逻辑日"（前一天）
        assert!(!needs_new_doc(
            Some("2026-05-19T02:00:00+08:00"),
            "2026-05-19T04:00:00+08:00"
        ));
    }

    #[test]
    fn test_needs_new_doc_cross_boundary() {
        // 凌晨 3 点（属于 5/18）→ 早上 7 点（属于 5/19）：新一天
        assert!(needs_new_doc(
            Some("2026-05-19T03:00:00+08:00"),
            "2026-05-19T07:00:00+08:00"
        ));
    }

    #[test]
    fn test_needs_new_doc_next_day_afternoon() {
        assert!(needs_new_doc(
            Some("2026-05-18T22:00:00+08:00"),
            "2026-05-19T14:00:00+08:00"
        ));
    }

    #[test]
    fn test_needs_new_doc_evening_to_late_night() {
        // 5/18 晚 11 点 → 5/19 凌晨 1 点：还是同一"逻辑日"（5/18）
        assert!(!needs_new_doc(
            Some("2026-05-18T23:00:00+08:00"),
            "2026-05-19T01:00:00+08:00"
        ));
    }
}
