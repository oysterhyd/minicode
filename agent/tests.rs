use crate::{
    attachments::Attachments,
    configuration::{self, Configuration},
    context,
    extensions::Catalog,
    mcp::McpRegistry,
    models::{Block, Message, Usage},
    store::Store,
    tools::{self, ToolContext},
    util,
};
use serde_json::json;
use std::{collections::BTreeMap, fs, sync::Arc};
use tokio_util::sync::CancellationToken;
fn empty_catalog(workspace: &std::path::Path, configuration: Configuration) -> Catalog {
    Catalog {
        workspace: workspace.into(),
        skills: BTreeMap::new(),
        plugins: vec![],
        servers: vec![],
        agents: BTreeMap::new(),
        local: vec![],
        instructions: BTreeMap::new(),
        active: BTreeMap::new(),
        pending_scopes: std::collections::HashSet::new(),
        configuration,
    }
}
fn fixture() -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = util::canonical(dir.path()).unwrap();
    let store = Store::open(workspace.join("test.db")).unwrap();
    let session = store
        .create(&workspace.to_string_lossy(), "fake", "fake")
        .unwrap();
    let catalog = empty_catalog(
        &workspace,
        Configuration {
            path: workspace.join("config.json"),
        },
    );
    let ctx = ToolContext {
        workspace: workspace.clone(),
        session,
        store,
        attachments: Attachments {
            root: workspace.join("attachments"),
        },
        modalities: vec!["image".into()],
        catalog: Arc::new(tokio::sync::Mutex::new(catalog)),
        mcp: Arc::new(McpRegistry::default()),
        cancel: CancellationToken::new(),
        output: None,
        deadline: None,
    };
    (dir, ctx)
}
#[test]
fn utf8_bounds() {
    for s in ["", "abc", "中文测试", "a😀b\n", "🦀🦀🦀"] {
        for n in 0..20 {
            assert!(util::prefix(s, n).len() <= n);
            assert!(s.starts_with(util::prefix(s, n)));
            assert!(util::suffix(s, n).len() <= n);
            assert!(s.ends_with(util::suffix(s, n)));
        }
    }
}
#[test]
fn usage_preserves_uncertainty() {
    let mut u = Usage::default();
    u.add(&Usage {
        input_tokens: 12,
        output_tokens: 3,
        cache_read_tokens: 2,
        cache_write_tokens: 1,
        available: false,
    });
    assert_eq!(u.total(), 15);
    assert!(!u.available);
}
#[test]
fn message_blocks_roundtrip() {
    let value = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"read","is_error":false,"images":[]},{"type":"text","text":"中文"}]});
    let message: Message = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), value);
}
#[test]
fn schema_bounds() {
    for (name, valid, invalid) in [
        ("read", json!({"path":"a"}), json!({"path":"a","offset":0})),
        (
            "edit",
            json!({"path":"a","old_text":"a","new_text":"b"}),
            json!({"path":"a","old_text":"","new_text":"b"}),
        ),
        (
            "task_complete",
            json!({"task_id":"a","ok":true}),
            json!({"task_id":"a","ok":"false"}),
        ),
    ] {
        let spec = tools::builtins().iter().find(|s| s.name == name).unwrap();
        assert!(tools::validate(spec, &valid).is_ok());
        assert!(tools::validate(spec, &invalid).is_err());
    }
}
#[test]
fn policies() {
    for mode in ["default", "accept_edits", "bypass"] {
        assert_eq!(tools::policy("read", mode), "allow");
    }
    assert_eq!(tools::policy("write", "default"), "ask");
    assert_eq!(tools::policy("write", "accept_edits"), "allow");
    assert_eq!(tools::policy("bash", "accept_edits"), "ask");
    assert_eq!(tools::policy("mcp__server__read", "default"), "ask");
    assert_eq!(tools::policy("unknown", "default"), "deny");
}
#[test]
fn directory_escape() {
    let (dir, ctx) = fixture();
    let outside = dir.path().parent().unwrap().join("outside");
    assert!(util::in_workspace(&ctx.workspace, "../outside").is_err());
    assert!(util::in_workspace(&ctx.workspace, &outside.to_string_lossy()).is_err());
    assert!(util::in_workspace(&ctx.workspace, "sub/new.txt").is_ok());
    let secret = dir.path().join("private.txt");
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(&secret, "private").unwrap();
    let root = util::canonical(&repo).unwrap();
    assert!(util::in_workspace(&root, "../private.txt").is_err());
}
#[test]
fn atomic_file_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("folder/a.txt");
    util::atomic_write(&path, b"one").unwrap();
    util::atomic_write(&path, "中文\r\n".as_bytes()).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "中文\r\n");
}
#[tokio::test]
async fn read_write_edit_search() {
    let (_dir, ctx) = fixture();
    let out = tools::execute(
        "write",
        &json!({"path":"a.txt","content":"one\ntwo\nthree\n"}),
        &ctx,
    )
    .await;
    assert!(out.success, "{:?}", out.error);
    assert!(out.output.starts_with("Applied change"));
    let out = tools::execute("read", &json!({"path":"a.txt","offset":2,"limit":1}), &ctx).await;
    assert!(out.success);
    assert!(out.output.starts_with("2\ttwo"));
    let out = tools::execute(
        "edit",
        &json!({"path":"a.txt","old_text":"two","new_text":"TWO"}),
        &ctx,
    )
    .await;
    assert!(out.success);
    assert!(out.output.contains("+TWO"));
    assert!(out.output.contains("\n-two\n+TWO\n"));
    let out = tools::execute("grep", &json!({"pattern":"TWO","path":"a.txt"}), &ctx).await;
    assert!(out.output.contains("a.txt:2: TWO"));
}
#[tokio::test]
async fn ambiguous_edit_is_atomic() {
    let (_dir, ctx) = fixture();
    fs::write(ctx.workspace.join("a.txt"), "x x").unwrap();
    let out = tools::execute(
        "edit",
        &json!({"path":"a.txt","old_text":"x","new_text":"y"}),
        &ctx,
    )
    .await;
    assert!(!out.success);
    assert_eq!(
        fs::read_to_string(ctx.workspace.join("a.txt")).unwrap(),
        "x x"
    );
    let out = tools::execute(
        "edit",
        &json!({"path":"a.txt","old_text":"x","new_text":"y","replace_all":true}),
        &ctx,
    )
    .await;
    assert!(out.success);
    assert_eq!(
        fs::read_to_string(ctx.workspace.join("a.txt")).unwrap(),
        "y y"
    );
}
#[tokio::test]
async fn large_edit_and_unicode_cursor() {
    let (_dir, ctx) = fixture();
    let text = format!("{}needle{}", "中文".repeat(60_000), "b".repeat(60_000));
    fs::write(ctx.workspace.join("large.txt"), &text).unwrap();
    let page = tools::execute("read", &json!({"path":"large.txt"}), &ctx).await;
    assert!(page.success);
    assert!(page.output.contains("next_offset=1"));
    let cursor = page
        .output
        .split("next_cursor=")
        .nth(1)
        .unwrap()
        .trim_end_matches(']')
        .parse::<u64>()
        .unwrap();
    let page2 = tools::execute("read", &json!({"path":"large.txt","cursor":cursor}), &ctx).await;
    assert!(page2.success);
    let edited = tools::execute(
        "edit",
        &json!({"path":"large.txt","old_text":"needle","new_text":"replacement"}),
        &ctx,
    )
    .await;
    assert!(edited.success);
    assert_eq!(
        fs::read_to_string(ctx.workspace.join("large.txt")).unwrap(),
        text.replace("needle", "replacement")
    );
}
#[tokio::test]
async fn non_utf8_and_path_errors() {
    let (_dir, ctx) = fixture();
    fs::write(ctx.workspace.join("binary.txt"), b"\xff\xfe").unwrap();
    assert!(
        !tools::execute("read", &json!({"path":"binary.txt"}), &ctx)
            .await
            .success
    );
    assert!(
        !tools::execute("write", &json!({"path":"../escape","content":"x"}), &ctx)
            .await
            .success
    );
    assert!(
        !tools::execute("read", &json!({"path":"not-found"}), &ctx)
            .await
            .success
    );
}
#[tokio::test]
async fn ls_pagination_skip_dirs() {
    let (_dir, ctx) = fixture();
    fs::create_dir_all(ctx.workspace.join("node_modules")).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(ctx.workspace.join(name), "").unwrap();
    }
    let out = tools::execute("ls", &json!({"path":".","limit":2}), &ctx).await;
    assert!(!out.output.contains("node_modules"));
    assert!(out.output.contains("next_offset=2"));
}
#[test]
fn transactions_and_journal() {
    let (_dir, ctx) = fixture();
    let message = Message::user("start");
    let result = Block::ToolResult {
        tool_use_id: "one".into(),
        content: "complete".into(),
        is_error: false,
        images: vec![],
    };
    let event = ctx
        .store
        .checkpoint(
            &ctx.session,
            "tool_call_result",
            &json!({"call_id":"one"}),
            Some(&message),
            Some(&result),
            Some(&json!({"rounds":2})),
        )
        .unwrap();
    assert_eq!(event.seq, 0);
    assert_eq!(ctx.store.messages(&ctx.session).unwrap(), vec![message]);
    assert_eq!(ctx.store.tool_results(&ctx.session).unwrap()["one"], result);
    assert_eq!(ctx.store.require(&ctx.session).unwrap()["rounds"], 2);
    let error: anyhow::Result<()> = ctx.store.transaction(|c| {
        c.execute("UPDATE sessions SET rounds=9", [])?;
        anyhow::bail!("rollback")
    });
    assert!(error.is_err());
    assert_eq!(ctx.store.require(&ctx.session).unwrap()["rounds"], 2);
}
#[test]
fn task_graph_claim_and_dependencies() {
    let (_dir, ctx) = fixture();
    ctx.store
        .task_create(&ctx.session, &json!({"task_id":"a","title":"a"}))
        .unwrap();
    ctx.store
        .task_create(
            &ctx.session,
            &json!({"task_id":"b","title":"b","depends_on":["a"]}),
        )
        .unwrap();
    assert!(ctx.store.task_claim(&ctx.session, "b", "agent").is_err());
    ctx.store.task_claim(&ctx.session, "a", "agent").unwrap();
    assert!(ctx.store.task_claim(&ctx.session, "a", "second").is_err());
    ctx.store.task_complete(&ctx.session, "a", true).unwrap();
    ctx.store.task_claim(&ctx.session, "b", "agent").unwrap();
    assert!(
        ctx.store
            .task_create(
                &ctx.session,
                &json!({"task_id":"c","title":"c","depends_on":["unknown"]})
            )
            .is_err()
    );
    assert_eq!(ctx.store.tasks(&ctx.session).unwrap().len(), 2);
}
#[test]
fn per_session_lock() {
    let (_dir, ctx) = fixture();
    let guard = ctx.store.activation(&ctx.session).unwrap();
    assert!(ctx.store.activation(&ctx.session).is_err());
    drop(guard);
    assert!(ctx.store.activation(&ctx.session).is_ok());
}
#[test]
fn attachments_digest_and_tamper() {
    let (_dir, ctx) = fixture();
    let block = ctx
        .attachments
        .import_bytes(b"\x89PNG\r\n\x1a\nexample", "example.png", "image/png")
        .unwrap();
    let Block::Media { asset_id, .. } = &block else {
        panic!()
    };
    assert_eq!(ctx.attachments.get(asset_id).unwrap(), block);
    assert!(ctx.attachments.encoded(&block).is_ok());
    fs::write(ctx.attachments.path(&block).unwrap(), b"tampered").unwrap();
    assert!(ctx.attachments.encoded(&block).is_err());
    assert!(ctx.attachments.get("../../outside").is_err());
    assert!(
        ctx.attachments
            .import_bytes(b"not png", "example.png", "image/png")
            .is_err()
    );
}
#[test]
fn config_credentials_are_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let configuration = Configuration {
        path: dir.path().join("config.json"),
    };
    let public=configuration.save_service(&json!({"name":"test","baseUrl":"https://example.test/v1","apiStyle":"openai","apiKey":"placeholder-only","enabled":true,"models":[{"modelId":"test","contextWindow":200000,"maxOutputTokens":8192}]})).unwrap();
    assert_eq!(public["services"][0]["apiKey"], "");
    assert_eq!(public["services"][0]["hasApiKey"], true);
    assert_eq!(
        configuration.read().unwrap()["services"][0]["apiKey"],
        "placeholder-only"
    );
    assert!(configuration.save_service(&json!({"name":"bad","baseUrl":"https://key@example.test/v1","apiStyle":"openai","models":[]})).is_err());
}
#[test]
fn strict_capabilities() {
    assert!(configuration::capabilities(&json!({"images":"true"})).is_err());
    assert_eq!(
        configuration::capabilities(&json!({"images":true})).unwrap()["audio"],
        false
    );
}
#[test]
fn complete_call_pairing() {
    let m = vec![
        Message::user("first"),
        Message {
            role: "assistant".into(),
            content: vec![Block::ToolUse {
                id: "a".into(),
                name: "read".into(),
                input: json!({}),
            }],
        },
        Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: "a".into(),
                content: "ok".into(),
                is_error: false,
                images: vec![],
            }],
        },
        Message::user("second"),
    ];
    assert_eq!(context::paired_prefix(&m, &[1, 2, 3, 4]), Some(4));
    assert_eq!(context::paired_prefix(&m, &[2]), None);
}
#[test]
fn duplicate_or_orphan_pairing_is_rejected() {
    let m = vec![
        Message {
            role: "assistant".into(),
            content: vec![
                Block::ToolUse {
                    id: "a".into(),
                    name: "read".into(),
                    input: json!({}),
                },
                Block::ToolUse {
                    id: "a".into(),
                    name: "read".into(),
                    input: json!({}),
                },
            ],
        },
        Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: "a".into(),
                content: "ok".into(),
                is_error: false,
                images: vec![],
            }],
        },
    ];
    assert_eq!(context::paired_prefix(&m, &[1, 2]), None);
}
#[test]
fn versioned_database_rejects_future_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA user_version=8")
        .unwrap();
    assert!(Store::open(path).is_err());
}
#[test]
fn goal_spec_validation() {
    assert!(serde_yaml::from_str::<crate::goals::Spec>("items: []\nunknown: true").is_err());
    let spec: crate::goals::Spec =
        serde_yaml::from_str("items:\n - id: run\n   type: command\n   command: node --version\n")
            .unwrap();
    assert!(spec.validate().is_ok());
}
#[test]
fn protected_baseline_survives_restore() {
    let (dir, ctx) = fixture();
    fs::write(dir.path().join("protected.txt"), "before").unwrap();
    let spec: crate::goals::Spec = serde_json::from_value(
        json!({"items":[{"id":"protect","type":"protected","path":"protected.txt"}]}),
    )
    .unwrap();
    let goal = crate::goals::Goal::new(spec, &ctx.workspace).unwrap();
    goal.save(&ctx.store, &ctx.session).unwrap();
    let restored = crate::goals::Goal::restore(
        &ctx.store.goal(&ctx.session).unwrap().unwrap(),
        &ctx.workspace,
    )
    .unwrap();
    assert_eq!(goal.protected, restored.protected);
}
#[test]
fn html_report_escapes_untrusted_content() {
    let summary = json!({"session_id":"s","workspace":"<script>unsafe</script>","model":"fake","provider":"fake","status":"completed","rounds":1});
    let html = crate::report::render(&summary, &[], &[], &Default::default());
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("目标验收（Goal Checks）"));
    assert!(html.contains("时间线"));
    assert!(html.contains("<style>"));
}
