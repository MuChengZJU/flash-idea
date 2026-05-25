use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Local, NaiveDate, NaiveTime, Utc};
use feishu_client::{FeishuClient, FeishuError};
use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::{sync::RwLock, time::sleep};

use crate::db::{self, Message};

const DAY_BOUNDARY: NaiveTime = match NaiveTime::from_hms_opt(6, 0, 0) {
    Some(t) => t,
    None => unreachable!(),
};

#[derive(Clone, Serialize)]
struct SyncStatusChanged {
    id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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

fn network_retry_backoff_delay(retry_count: i64) -> Option<Duration> {
    match retry_count {
        1..=5 => Some(Duration::from_secs(1 << (retry_count - 1))),
        _ => None,
    }
}

fn resolve_pull_doc_id(
    found_daily_doc_id: Option<String>,
    stored_doc_id: Option<String>,
) -> Option<String> {
    found_daily_doc_id.or(stored_doc_id)
}

async fn resolve_doc_id(
    feishu_client: &FeishuClient,
    db: &Mutex<Connection>,
    wiki: &WikiConfig,
    message: &Message,
) -> Result<String, FeishuError> {
    let (active_doc, last_synced) = {
        eprintln!(
            "resolve_doc_id: message_id={} checking active_doc_id and last synced message",
            message.id
        );
        let conn = db.lock().map_err(|e| FeishuError::ApiError {
            code: -1,
            msg: e.to_string(),
        })?;
        let doc = match db::get_setting(&conn, "active_doc_id") {
            Ok(value) => value,
            Err(err) => {
                eprintln!(
                    "resolve_doc_id: message_id={} failed reading active_doc_id: {}",
                    message.id, err
                );
                None
            }
        };
        let last = match db::get_last_synced_at(&conn) {
            Ok(value) => value,
            Err(err) => {
                eprintln!(
                    "resolve_doc_id: message_id={} failed reading last synced message: {}",
                    message.id, err
                );
                None
            }
        };
        eprintln!(
            "resolve_doc_id: message_id={} active_doc_id={:?} last_synced_at={:?}",
            message.id, doc, last
        );
        (doc, last)
    };

    if let Some(ref doc_id) = active_doc {
        let create_new_doc = needs_new_doc(last_synced.as_deref(), &message.created_at);
        eprintln!(
            "resolve_doc_id: message_id={} needs_new_doc={} active_doc_id={} message_created_at={}",
            message.id, create_new_doc, doc_id, message.created_at
        );
        if !create_new_doc {
            eprintln!(
                "resolve_doc_id: message_id={} reusing active_doc_id={}",
                message.id, doc_id
            );
            return Ok(doc_id.clone());
        }
    } else {
        eprintln!(
            "resolve_doc_id: message_id={} no active_doc_id found; will create wiki child",
            message.id
        );
    }

    let now = DateTime::parse_from_rfc3339(&message.created_at)
        .unwrap_or_else(|_| Utc::now().fixed_offset())
        .with_timezone(&Local);

    let doc_date = if now.time() < DAY_BOUNDARY {
        now.date_naive() - chrono::Duration::days(1)
    } else {
        now.date_naive()
    };
    let title = format!("Flash Idea - {}", doc_date.format("%Y-%m-%d"));
    let legacy_title = format!("FlashIdea - {}", doc_date.format("%Y-%m-%d"));

    let new_doc_id = match find_existing_daily_doc(feishu_client, wiki, &[&title, &legacy_title], &message.id).await
    {
        Some(obj_token) => {
            eprintln!(
                "resolve_doc_id: message_id={} found existing doc obj_token={} for title={}",
                message.id, obj_token, title
            );
            obj_token
        }
        None => {
            eprintln!(
                "resolve_doc_id: message_id={} create_wiki_child attempt space_id={} parent_node_token={} title={}",
                message.id, wiki.space_id, wiki.node_token, title
            );
            let node = match feishu_client
                .create_wiki_child(&wiki.space_id, &wiki.node_token, &title)
                .await
            {
                Ok(node) => {
                    eprintln!(
                        "resolve_doc_id: message_id={} create_wiki_child succeeded node_token={} obj_token={} obj_type={}",
                        message.id, node.node_token, node.obj_token, node.obj_type
                    );
                    node
                }
                Err(err) => {
                    eprintln!(
                        "resolve_doc_id: message_id={} create_wiki_child failed: {:?}",
                        message.id, err
                    );
                    return Err(err);
                }
            };
            node.obj_token
        }
    };
    match db.lock() {
        Ok(conn) => {
            if let Err(err) = db::set_setting(&conn, "active_doc_id", &new_doc_id) {
                eprintln!(
                    "resolve_doc_id: message_id={} failed saving active_doc_id={}: {}",
                    message.id, new_doc_id, err
                );
            }
        }
        Err(err) => {
            eprintln!(
                "resolve_doc_id: message_id={} failed locking db to save active_doc_id={}: {}",
                message.id, new_doc_id, err
            );
        }
    }

    Ok(new_doc_id)
}

async fn find_existing_daily_doc(
    feishu_client: &FeishuClient,
    wiki: &WikiConfig,
    titles: &[&str],
    message_id: &str,
) -> Option<String> {
    match feishu_client
        .list_wiki_children(&wiki.space_id, &wiki.node_token)
        .await
    {
        Ok(children) => {
            for title in titles {
                for child in &children {
                    if child.title == *title {
                        return Some(child.obj_token.clone());
                    }
                }
            }
            eprintln!(
                "find_existing_daily_doc: message_id={} no match for titles={:?} among {} children",
                message_id,
                titles,
                children.len()
            );
            None
        }
        Err(err) => {
            eprintln!(
                "find_existing_daily_doc: message_id={} list_wiki_children failed: {:?}, will create new doc",
                message_id, err
            );
            None
        }
    }
}

pub async fn pull_remote_messages(
    feishu_client: Arc<RwLock<Arc<FeishuClient>>>,
    db: Arc<Mutex<Connection>>,
    wiki: Arc<RwLock<Option<Arc<WikiConfig>>>>,
    app_handle: AppHandle,
) {
    let client = {
        let guard = feishu_client.read().await;
        Arc::clone(&*guard)
    };
    let wiki = {
        let guard = wiki.read().await;
        guard.clone()
    };
    let wiki = match wiki {
        Some(w) => w,
        None => return,
    };

    let now = Local::now();
    let doc_date = if now.time() < DAY_BOUNDARY {
        now.date_naive() - chrono::Duration::days(1)
    } else {
        now.date_naive()
    };
    let title = format!("Flash Idea - {}", doc_date.format("%Y-%m-%d"));
    let legacy_title = format!("FlashIdea - {}", doc_date.format("%Y-%m-%d"));

    let stored = if let Ok(conn) = db.lock() {
        db::get_setting(&conn, "active_doc_id").ok().flatten()
    } else {
        None
    };
    // Pull must validate today's wiki doc first; a stored active_doc_id can point
    // at yesterday's doc after a day boundary and would misdate pulled messages.
    let found = find_existing_daily_doc(&client, &wiki, &[&title, &legacy_title], "pull").await;
    if let Some(ref id) = found {
        if stored.as_deref() != Some(id.as_str()) {
            if let Ok(conn) = db.lock() {
                let _ = db::set_setting(&conn, "active_doc_id", id);
            }
        }
    }
    let doc_id = match resolve_pull_doc_id(found, stored) {
        Some(id) => id,
        None => {
            eprintln!("pull_remote_messages: no existing doc for {title}, skip pull");
            return;
        }
    };

    let raw_content = match client.get_document_raw_content(&doc_id).await {
        Ok(c) => c,
        Err(err) => {
            eprintln!("pull_remote_messages: get_document_raw_content failed: {err:?}");
            return;
        }
    };

    let parsed = parse_remote_lines(&raw_content, doc_date);
    if parsed.is_empty() {
        return;
    }

    let now_str = Utc::now().to_rfc3339();
    let mut inserted = 0u32;

    if let Ok(conn) = db.lock() {
        for (text, created_at) in &parsed {
            let id = uuid::Uuid::new_v4().to_string();
            match db::insert_remote_message(&conn, &id, text, created_at, &doc_id, &now_str) {
                Ok(true) => inserted += 1,
                Ok(false) => {}
                Err(err) => {
                    eprintln!("pull_remote_messages: insert failed for text={}: {err}", text);
                }
            }
        }
    }

    if inserted > 0 {
        eprintln!(
            "pull_remote_messages: pulled {inserted} new messages from doc {doc_id}"
        );
        let _ = app_handle.emit("messages_updated", inserted);
    }
}

fn parse_remote_lines(raw: &str, doc_date: NaiveDate) -> Vec<(String, String)> {
    let mut results = Vec::new();
    let local_offset = Local::now().offset().clone();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let time_str = &rest[..bracket_end];
                let text = rest[bracket_end + 1..].trim_start().to_string();
                if text.is_empty() {
                    continue;
                }

                if let Ok(time) = NaiveTime::parse_from_str(time_str, "%H:%M:%S") {
                    let calendar_date = if time < DAY_BOUNDARY {
                        doc_date + chrono::Duration::days(1)
                    } else {
                        doc_date
                    };
                    let naive_dt = calendar_date.and_time(time);
                    let local_dt = naive_dt.and_local_timezone(local_offset).single();
                    let created_at = match local_dt {
                        Some(dt) => dt.to_rfc3339(),
                        None => Utc::now().to_rfc3339(),
                    };
                    results.push((text, created_at));
                }
            }
        }
    }

    results
}

pub async fn sync_message(
    feishu_client: Arc<RwLock<Arc<FeishuClient>>>,
    db: Arc<Mutex<Connection>>,
    wiki: Arc<RwLock<Option<Arc<WikiConfig>>>>,
    fallback_doc_id: Arc<RwLock<String>>,
    message: Message,
    app_handle: AppHandle,
) {
    let feishu_client = {
        let guard = feishu_client.read().await;
        Arc::clone(&*guard)
    };
    let wiki = {
        let guard = wiki.read().await;
        guard.clone()
    };
    let fallback_doc_id = {
        let guard = fallback_doc_id.read().await;
        guard.clone()
    };

    let doc_id = if let Some(ref wiki) = wiki {
        match resolve_doc_id(&feishu_client, &db, wiki, &message).await {
            Ok(id) => id,
            Err(err) => {
                let reason = user_friendly_error(&err);
                eprintln!(
                    "sync_message: message_id={} resolve_doc_id failed: {:?}",
                    message.id, err
                );
                if fallback_doc_id.trim().is_empty() {
                    let reason = format!("文档定位失败: {reason}");
                    if let Ok(conn) = db.lock() {
                        let _ = db::update_sync_status(&conn, &message.id, "failed", None, Some(&reason));
                        let _ = db::insert_log(&conn, "error", "sync", &format!("msg={} {reason}", message.id));
                    }
                    emit_status(&app_handle, &message.id, "failed", Some(&reason));
                    return;
                }
                fallback_doc_id.clone()
            }
        }
    } else if !fallback_doc_id.trim().is_empty() {
        fallback_doc_id.clone()
    } else {
        let reason = "知识库未配置，请在设置中填写知识库节点 Token";
        if let Ok(conn) = db.lock() {
            let _ = db::update_sync_status(&conn, &message.id, "failed", None, Some(reason));
            let _ = db::insert_log(&conn, "error", "sync", &format!("msg={} {reason}", message.id));
        }
        emit_status(&app_handle, &message.id, "failed", Some(reason));
        return;
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
                    let _ = db::update_sync_status(&conn, &message.id, "synced", Some(&synced_at), None);
                }
                emit_status(&app_handle, &message.id, "synced", None);
                return;
            }
            Err(FeishuError::RateLimited) if !rate_limited_once => {
                rate_limited_once = true;
                sleep(Duration::from_millis(350)).await;
            }
            Err(err @ FeishuError::NetworkError(_)) => {
                let retry_count = if let Ok(conn) = db.lock() {
                    db::increment_retry(&conn, &message.id).unwrap_or(message.retry_count + 1)
                } else {
                    message.retry_count + 1
                };

                // Keep network failures queued inside this task: persist retry
                // count, wait with exponential backoff, then try append again.
                if let Some(delay) = network_retry_backoff_delay(retry_count) {
                    sleep(delay).await;
                    continue;
                }

                let reason = user_friendly_error(&err);
                eprintln!(
                    "sync_message: message_id={} failed after 5 retries: {:?}",
                    message.id, err
                );
                if let Ok(conn) = db.lock() {
                    let _ =
                        db::update_sync_status(&conn, &message.id, "failed", None, Some(&reason));
                    let _ = db::insert_log(
                        &conn,
                        "error",
                        "sync",
                        &format!("msg={} {reason} (retries=5)", message.id),
                    );
                }
                emit_status(&app_handle, &message.id, "failed", Some(&reason));
                return;
            }
            Err(ref err @ FeishuError::RateLimited)
            | Err(ref err @ FeishuError::AuthError(_))
            | Err(ref err @ FeishuError::ApiError { .. }) => {
                let reason = user_friendly_error(err);
                eprintln!(
                    "sync_message: message_id={} failed: {:?}",
                    message.id, err
                );
                if let Ok(conn) = db.lock() {
                    let _ = db::update_sync_status(&conn, &message.id, "failed", None, Some(&reason));
                    let _ = db::insert_log(&conn, "error", "sync", &format!("msg={} {reason}", message.id));
                }
                emit_status(&app_handle, &message.id, "failed", Some(&reason));
                return;
            }
        }
    }
}

pub async fn sync_all_queued(
    feishu_client: Arc<RwLock<Arc<FeishuClient>>>,
    db: Arc<Mutex<Connection>>,
    wiki: Arc<RwLock<Option<Arc<WikiConfig>>>>,
    fallback_doc_id: Arc<RwLock<String>>,
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
            Arc::clone(&wiki),
            Arc::clone(&fallback_doc_id),
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

fn emit_status(app_handle: &AppHandle, id: &str, status: &str, error: Option<&str>) {
    let _ = app_handle.emit(
        "sync_status_changed",
        SyncStatusChanged {
            id: id.to_string(),
            status: status.to_string(),
            error: error.map(String::from),
        },
    );
}

fn user_friendly_error(err: &FeishuError) -> String {
    match err {
        FeishuError::AuthError(_) => "认证失败，请检查 App ID 和 App Secret".to_string(),
        FeishuError::NetworkError(_) => "网络连接失败，请检查网络".to_string(),
        FeishuError::RateLimited => "飞书接口限流，请稍后重试".to_string(),
        FeishuError::ApiError { code, msg } => format!("飞书接口错误 {code}: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_retry_backoff_sequence() {
        let delays: Vec<_> = (1..=5)
            .map(network_retry_backoff_delay)
            .collect::<Option<Vec<_>>>()
            .expect("five network failures should have backoff delays");

        assert_eq!(
            delays,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
            ]
        );
        assert_eq!(network_retry_backoff_delay(6), None);
    }

    #[test]
    fn test_pull_doc_resolution_prefers_today_search_over_stored_id() {
        assert_eq!(
            resolve_pull_doc_id(
                Some("today-doc".to_string()),
                Some("yesterday-doc".to_string())
            ),
            Some("today-doc".to_string())
        );
    }

    #[test]
    fn test_pull_doc_resolution_falls_back_to_stored_id() {
        assert_eq!(
            resolve_pull_doc_id(None, Some("stored-doc".to_string())),
            Some("stored-doc".to_string())
        );
        assert_eq!(resolve_pull_doc_id(None, None), None);
    }

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

    #[test]
    fn test_parse_remote_lines_basic() {
        let raw = "[10:00:01] hello world\n[10:05:32] second message\n";
        let doc_date = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
        let parsed = parse_remote_lines(raw, doc_date);

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "hello world");
        assert_eq!(parsed[1].0, "second message");
        assert!(parsed[0].1.contains("2026-05-19"));
    }

    #[test]
    fn test_parse_remote_lines_early_morning() {
        let raw = "[02:30:00] late night thought\n";
        let doc_date = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
        let parsed = parse_remote_lines(raw, doc_date);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "late night thought");
        assert!(parsed[0].1.contains("2026-05-20"));
    }

    #[test]
    fn test_parse_remote_lines_skips_empty() {
        let raw = "\n\n[10:00:00] valid\n\n[bad line\n";
        let doc_date = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
        let parsed = parse_remote_lines(raw, doc_date);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "valid");
    }
}
