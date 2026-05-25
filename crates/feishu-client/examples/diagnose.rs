use feishu_client::FeishuClient;
use std::env;

fn load_env() {
    let mut dir = env::current_dir().expect("current_dir");
    loop {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            dotenvy::from_path(&candidate).ok();
            eprintln!("[env] loaded {}", candidate.display());
            return;
        }
        if !dir.pop() {
            break;
        }
    }
    eprintln!("[env] no .env found");
}

#[tokio::main]
async fn main() {
    load_env();

    let app_id = env::var("FEISHU_APP_ID").unwrap_or_default();
    let app_secret = env::var("FEISHU_APP_SECRET").unwrap_or_default();
    let wiki_node_token = env::var("FEISHU_WIKI_NODE_TOKEN").unwrap_or_default();

    if app_id.is_empty() || app_secret.is_empty() {
        eprintln!("[FAIL] FEISHU_APP_ID or FEISHU_APP_SECRET is empty");
        return;
    }
    eprintln!("[ok] FEISHU_APP_ID={} (len={})", &app_id[..6], app_id.len());

    let client = FeishuClient::new(app_id, app_secret);

    // Step 1: test token
    eprintln!("\n--- Step 1: get tenant_access_token ---");
    let token_result = client.append_text("__dummy__", "test", "diag-token-check").await;
    match &token_result {
        Err(feishu_client::FeishuError::AuthError(msg)) => {
            eprintln!("[FAIL] token error: {msg}");
            eprintln!("检查 FEISHU_APP_ID / FEISHU_APP_SECRET 是否正确，应用是否已发布");
            return;
        }
        Err(feishu_client::FeishuError::ApiError { code, msg }) => {
            eprintln!("[ok] token works (got API error on dummy doc, which is expected)");
            eprintln!("     code={code} msg={msg}");
        }
        Err(feishu_client::FeishuError::NetworkError(msg)) => {
            eprintln!("[FAIL] network error: {msg}");
            return;
        }
        _ => {
            eprintln!("[ok] token works (unexpected success on dummy doc)");
        }
    }

    // Step 2: get wiki node
    if wiki_node_token.is_empty() {
        eprintln!("\n--- Step 2: SKIPPED (no FEISHU_WIKI_NODE_TOKEN) ---");
        return;
    }
    eprintln!("\n--- Step 2: get_wiki_node({wiki_node_token}) ---");
    let node = match client.get_wiki_node(&wiki_node_token).await {
        Ok(node) => {
            eprintln!("[ok] space_id={}", node.space_id);
            eprintln!("     node_token={}", node.node_token);
            eprintln!("     obj_token={}", node.obj_token);
            eprintln!("     obj_type={}", node.obj_type);
            eprintln!("     title={}", node.title);
            node
        }
        Err(e) => {
            eprintln!("[FAIL] get_wiki_node error: {e:?}");
            eprintln!("检查：应用是否有 wiki:wiki 权限？知识库是否把应用加为成员？");
            return;
        }
    };

    // Step 3: create wiki child
    eprintln!("\n--- Step 3: create_wiki_child ---");
    let title = "Flash Idea - diagnose-test";
    match client
        .create_wiki_child(&node.space_id, &wiki_node_token, title)
        .await
    {
        Ok(child) => {
            eprintln!("[ok] created child doc");
            eprintln!("     node_token={}", child.node_token);
            eprintln!("     obj_token={}", child.obj_token);
            eprintln!("     obj_type={}", child.obj_type);

            // Step 4: append text
            eprintln!("\n--- Step 4: append_text to new doc ---");
            match client
                .append_text(&child.obj_token, "diagnose test content", "diag-uuid")
                .await
            {
                Ok(()) => eprintln!("[ok] append_text succeeded!"),
                Err(e) => {
                    eprintln!("[FAIL] append_text error: {e:?}");
                    eprintln!("文档创建成功但写入失败，检查 docx:document 权限");
                }
            }
        }
        Err(e) => {
            eprintln!("[FAIL] create_wiki_child error: {e:?}");
            eprintln!("检查：应用在知识库中是否有编辑权限（不能只是阅读权限）？");
        }
    }

    eprintln!("\n--- 诊断完成 ---");
}
