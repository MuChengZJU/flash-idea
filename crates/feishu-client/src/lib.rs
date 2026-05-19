use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::RwLock;

const DEFAULT_BASE_URL: &str = "https://open.feishu.cn";
const TOKEN_REFRESH_BUFFER: Duration = Duration::from_secs(30 * 60);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct FeishuClient {
    app_id: String,
    app_secret: String,
    base_url: String,
    http_client: reqwest::Client,
    token_cache: RwLock<Option<CachedToken>>,
    #[cfg(test)]
    mock_transport: Option<Arc<MockTransport>>,
}

struct CachedToken {
    token: String,
    expires_at: Instant,
}

#[derive(Deserialize)]
struct TokenResponse {
    code: i64,
    msg: String,
    tenant_access_token: Option<String>,
    expire: Option<u64>,
}

#[derive(Deserialize)]
struct ApiResponse {
    code: i64,
    msg: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WikiNode {
    pub space_id: String,
    pub node_token: String,
    pub obj_token: String,
    pub obj_type: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Deserialize)]
struct WikiNodeResponse {
    code: i64,
    msg: String,
    data: Option<WikiNodeResponseData>,
}

#[derive(Deserialize)]
struct WikiNodeResponseData {
    node: Option<WikiNode>,
}

#[cfg(test)]
#[derive(Clone)]
struct MockResponse {
    status: u16,
    body: String,
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct CapturedRequest {
    path: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
}

#[cfg(test)]
struct MockTransport {
    responses: Mutex<Vec<MockResponse>>,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
}

impl FeishuClient {
    /// 创建客户端实例
    pub fn new(app_id: String, app_secret: String) -> Self {
        Self::new_with_base_url(app_id, app_secret, DEFAULT_BASE_URL.to_string())
    }

    fn new_with_base_url(app_id: String, app_secret: String, base_url: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(CLIENT_TIMEOUT)
            .build()
            .expect("reqwest client configuration is valid");

        Self {
            app_id,
            app_secret,
            base_url: base_url.trim_end_matches('/').to_string(),
            http_client,
            token_cache: RwLock::new(None),
            #[cfg(test)]
            mock_transport: None,
        }
    }

    #[cfg(test)]
    fn new_with_mock_responses(
        app_id: String,
        app_secret: String,
        responses: Vec<MockResponse>,
    ) -> (Self, Arc<Mutex<Vec<CapturedRequest>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut client = Self::new_with_base_url(app_id, app_secret, DEFAULT_BASE_URL.to_string());
        client.mock_transport = Some(Arc::new(MockTransport {
            responses: Mutex::new(responses),
            captured: captured.clone(),
        }));
        (client, captured)
    }

    /// 向指定文档追加一个文本段落
    /// client_token 用于幂等（传入消息 UUID）
    /// 返回 Ok(()) 或具体错误类型
    pub async fn append_text(
        &self,
        document_id: &str,
        content: &str,
        client_token: &str,
    ) -> Result<(), FeishuError> {
        let token = self.get_token().await?;
        let url = format!(
            "{}/open-apis/docx/v1/documents/{}/blocks/{}/children",
            self.base_url, document_id, document_id
        );
        let body = json!({
            "children": [{
                "block_type": 2,
                "text": {
                    "elements": [{
                        "text_run": {
                            "content": content,
                            "text_element_style": {}
                        }
                    }],
                    "style": {}
                }
            }]
        });

        let (status, response_body) = self
            .post_json(
                &url,
                &[("authorization", format!("Bearer {token}"))],
                &[("client_token", client_token)],
                body,
            )
            .await?;
        let api_response =
            serde_json::from_value::<ApiResponse>(response_body).unwrap_or_else(|_| ApiResponse {
                code: status.as_u16() as i64,
                msg: status.to_string(),
            });

        if status == StatusCode::UNAUTHORIZED {
            self.clear_token().await;
            return Err(FeishuError::AuthError(api_response.msg));
        }

        if status == StatusCode::TOO_MANY_REQUESTS || api_response.code == 99991400 {
            return Err(FeishuError::RateLimited);
        }

        if !status.is_success() || api_response.code != 0 {
            return Err(FeishuError::ApiError {
                code: api_response.code,
                msg: api_response.msg,
            });
        }

        Ok(())
    }

    pub async fn get_wiki_node(&self, node_token: &str) -> Result<WikiNode, FeishuError> {
        let token = self.get_token().await?;
        let url = format!(
            "{}/open-apis/wiki/v2/spaces/get_node",
            self.base_url
        );
        let (status, body) = self
            .get_json(
                &url,
                &[("authorization", format!("Bearer {token}"))],
                &[("token", node_token)],
            )
            .await?;
        let resp = serde_json::from_value::<WikiNodeResponse>(body)
            .map_err(|e| FeishuError::ApiError { code: -1, msg: e.to_string() })?;

        if !status.is_success() || resp.code != 0 {
            return Err(FeishuError::ApiError { code: resp.code, msg: resp.msg });
        }

        resp.data
            .and_then(|d| d.node)
            .ok_or_else(|| FeishuError::ApiError { code: -1, msg: "missing node in response".into() })
    }

    pub async fn create_wiki_child(
        &self,
        space_id: &str,
        parent_node_token: &str,
        title: &str,
    ) -> Result<WikiNode, FeishuError> {
        let token = self.get_token().await?;
        let url = format!(
            "{}/open-apis/wiki/v2/spaces/{}/nodes",
            self.base_url, space_id
        );
        let (status, body) = self
            .post_json(
                &url,
                &[("authorization", format!("Bearer {token}"))],
                &[],
                json!({
                    "obj_type": "docx",
                    "parent_node_token": parent_node_token,
                    "title": title,
                }),
            )
            .await?;
        let resp = serde_json::from_value::<WikiNodeResponse>(body)
            .map_err(|e| FeishuError::ApiError { code: -1, msg: e.to_string() })?;

        if !status.is_success() || resp.code != 0 {
            return Err(FeishuError::ApiError { code: resp.code, msg: resp.msg });
        }

        resp.data
            .and_then(|d| d.node)
            .ok_or_else(|| FeishuError::ApiError { code: -1, msg: "missing node in response".into() })
    }

    async fn get_token(&self) -> Result<String, FeishuError> {
        if let Some(cached) = self.token_cache.read().await.as_ref() {
            if cached.expires_at > Instant::now() + TOKEN_REFRESH_BUFFER {
                return Ok(cached.token.clone());
            }
        }

        let mut cache = self.token_cache.write().await;
        if let Some(cached) = cache.as_ref() {
            if cached.expires_at > Instant::now() + TOKEN_REFRESH_BUFFER {
                return Ok(cached.token.clone());
            }
        }

        let url = format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            self.base_url
        );
        let (status, response_body) = self
            .post_json(
                &url,
                &[],
                &[],
                json!({
                    "app_id": &self.app_id,
                    "app_secret": &self.app_secret,
                }),
            )
            .await?;
        let token_response = serde_json::from_value::<TokenResponse>(response_body)
            .map_err(|err| FeishuError::AuthError(err.to_string()))?;

        if !status.is_success() || token_response.code != 0 {
            return Err(FeishuError::AuthError(token_response.msg));
        }

        let token = token_response
            .tenant_access_token
            .ok_or_else(|| FeishuError::AuthError("missing tenant_access_token".to_string()))?;
        let expire = token_response.expire.unwrap_or(0);
        *cache = Some(CachedToken {
            token: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expire),
        });

        Ok(token)
    }

    async fn clear_token(&self) {
        *self.token_cache.write().await = None;
    }

    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        query: &[(&str, &str)],
        body: serde_json::Value,
    ) -> Result<(StatusCode, serde_json::Value), FeishuError> {
        #[cfg(test)]
        if let Some(mock) = &self.mock_transport {
            let mut path = url
                .strip_prefix(&self.base_url)
                .unwrap_or(url)
                .to_string();
            if !query.is_empty() {
                let query = query
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join("&");
                path.push('?');
                path.push_str(&query);
            }
            mock.captured.lock().unwrap().push(CapturedRequest {
                path,
                headers: headers
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.clone()))
                    .collect(),
                body,
            });
            let response = mock.responses.lock().unwrap().remove(0);
            let status = StatusCode::from_u16(response.status).unwrap();
            let body = serde_json::from_str(&response.body).unwrap_or_else(|_| json!({}));
            return Ok((status, body));
        }

        let mut request = self.http_client.post(url).json(&body);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if !query.is_empty() {
            request = request.query(query);
        }
        let response = request.send().await.map_err(map_reqwest_error)?;
        let status = response.status();
        let body = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|_| json!({}));
        Ok((status, body))
    }

    async fn get_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        query: &[(&str, &str)],
    ) -> Result<(StatusCode, serde_json::Value), FeishuError> {
        #[cfg(test)]
        if let Some(mock) = &self.mock_transport {
            let mut path = url
                .strip_prefix(&self.base_url)
                .unwrap_or(url)
                .to_string();
            if !query.is_empty() {
                let qs = query
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join("&");
                path.push('?');
                path.push_str(&qs);
            }
            mock.captured.lock().unwrap().push(CapturedRequest {
                path,
                headers: headers
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.clone()))
                    .collect(),
                body: json!(null),
            });
            let response = mock.responses.lock().unwrap().remove(0);
            let status = StatusCode::from_u16(response.status).unwrap();
            let body = serde_json::from_str(&response.body).unwrap_or_else(|_| json!({}));
            return Ok((status, body));
        }

        let mut request = self.http_client.get(url);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if !query.is_empty() {
            request = request.query(query);
        }
        let response = request.send().await.map_err(map_reqwest_error)?;
        let status = response.status();
        let body = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|_| json!({}));
        Ok((status, body))
    }
}

fn map_reqwest_error(err: reqwest::Error) -> FeishuError {
    if err.is_timeout() || err.is_connect() || err.is_request() {
        FeishuError::NetworkError(err.to_string())
    } else {
        FeishuError::ApiError {
            code: -1,
            msg: err.to_string(),
        }
    }
}

#[derive(Debug, Error)]
pub enum FeishuError {
    /// token 获取/刷新失败
    #[error("auth error: {0}")]
    AuthError(String),
    /// 限频（HTTP 429 或 99991400），调用方应重试
    #[error("rate limited")]
    RateLimited,
    /// 网络错误，调用方应重试
    #[error("network error: {0}")]
    NetworkError(String),
    /// API 返回的业务错误（文档不存在、权限不足等），不应重试
    #[error("api error {code}: {msg}")]
    ApiError { code: i64, msg: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn token_response(token: &str) -> String {
        json!({
            "code": 0,
            "msg": "ok",
            "tenant_access_token": token,
            "expire": 7200
        })
        .to_string()
    }

    #[test]
    fn test_client_creation() {
        let _client = FeishuClient::new("app-id".to_string(), "app-secret".to_string());
    }

    #[tokio::test]
    async fn test_append_text_request_body() {
        let (client, captured) = FeishuClient::new_with_mock_responses(
            "app-id".to_string(),
            "app-secret".to_string(),
            vec![
                MockResponse {
                    status: 200,
                    body: token_response("tenant-token"),
                },
                MockResponse {
                    status: 200,
                    body: r#"{"code":0,"msg":"ok"}"#.to_string(),
                },
            ],
        );

        client
            .append_text("doc-123", "hello feishu", "message-uuid")
            .await
            .unwrap();

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let append = &captured[1];
        assert_eq!(
            append.path,
            "/open-apis/docx/v1/documents/doc-123/blocks/doc-123/children?client_token=message-uuid"
        );
        assert!(append
            .headers
            .contains(&("authorization".to_string(), "Bearer tenant-token".to_string())));
        assert_eq!(
            append.body,
            json!({
                "children": [{
                    "block_type": 2,
                    "text": {
                        "elements": [{
                            "text_run": {
                                "content": "hello feishu",
                                "text_element_style": {}
                            }
                        }],
                        "style": {}
                    }
                }]
            })
        );
    }

    #[tokio::test]
    async fn test_token_caching() {
        let (client, captured) = FeishuClient::new_with_mock_responses(
            "app-id".to_string(),
            "app-secret".to_string(),
            vec![
                MockResponse {
                    status: 200,
                    body: token_response("cached-token"),
                },
                MockResponse {
                    status: 200,
                    body: r#"{"code":0,"msg":"ok"}"#.to_string(),
                },
                MockResponse {
                    status: 200,
                    body: r#"{"code":0,"msg":"ok"}"#.to_string(),
                },
            ],
        );

        client.append_text("doc-123", "first", "uuid-1").await.unwrap();
        client.append_text("doc-123", "second", "uuid-2").await.unwrap();

        let captured = captured.lock().unwrap();
        let token_requests = captured
            .iter()
            .filter(|request| request.path == "/open-apis/auth/v3/tenant_access_token/internal")
            .count();
        assert_eq!(token_requests, 1);
        assert_eq!(captured.len(), 3);
    }

    #[tokio::test]
    async fn test_error_mapping() {
        let (client, _captured) = FeishuClient::new_with_mock_responses(
            "app-id".to_string(),
            "app-secret".to_string(),
            vec![
                MockResponse {
                    status: 200,
                    body: token_response("error-token"),
                },
                MockResponse {
                    status: 401,
                    body: r#"{"code":99991663,"msg":"unauthorized"}"#.to_string(),
                },
                MockResponse {
                    status: 200,
                    body: token_response("new-error-token"),
                },
                MockResponse {
                    status: 429,
                    body: r#"{"code":99991400,"msg":"too many requests"}"#.to_string(),
                },
            ],
        );
        let err = client
            .append_text("unauthorized", "content", "uuid-401")
            .await
            .unwrap_err();
        assert!(matches!(err, FeishuError::AuthError(_)));

        let err = client
            .append_text("rate-limited", "content", "uuid-429")
            .await
            .unwrap_err();
        assert!(matches!(err, FeishuError::RateLimited));

        let network_client = FeishuClient::new_with_base_url(
            "app-id".to_string(),
            "app-secret".to_string(),
            "http://127.0.0.1:9".to_string(),
        );
        let err = network_client
            .append_text("doc-123", "content", "uuid-network")
            .await
            .unwrap_err();
        assert!(matches!(err, FeishuError::NetworkError(_)));
    }
}
