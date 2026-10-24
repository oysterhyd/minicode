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
fn small_context_windows_do_not_compact_short_histories() {
    let (_dir, ctx) = fixture();
    let mut messages = vec![Message::user("keep recent file results")];
    for i in 0..4 {
        messages.push(Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: format!("read-{i}"),
                content: "file contents\n".repeat(30),
                is_error: false,
                images: vec![],
            }],
        });
    }
    let original = messages.clone();
    assert!(
        context::compact(
            &ctx.store,
            &ctx.session,
            "",
            &[],
            &mut messages,
            8192,
            false
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(messages, original);
    assert!(
        context::compact(&ctx.store, &ctx.session, "", &[], &mut messages, 512, false)
            .unwrap()
            .is_some()
    );
    assert!(context::estimate("", &messages, &[]) < context::estimate("", &original, &[]));
    assert_eq!(&messages[3..], &original[3..]);
}
#[test]
fn patch_updates_keep_existing_plugins_compatible() {
    for (required, installed, compatible) in [
        ("2.0.0", "2.0.1", true),
        ("2.0.1", "2.0.1", true),
        ("2.0.2", "2.0.1", false),
        ("2.1.0", "2.0.1", false),
        ("1.3.2", "2.0.1", false),
        ("invalid", "2.0.1", false),
    ] {
        assert_eq!(
            crate::extensions::compatible_version(required, installed),
            compatible
        );
    }
}
#[tokio::test]
async fn duplicate_imported_mcp_servers_have_one_diagnostic() {
    let (_dir, ctx) = fixture();
    let source = ctx.workspace.join(".mcp.json");
    fs::write(&source, "{}").unwrap();
    let server = crate::extensions::Server {
        name: "same-source".into(),
        command: "minicode-missing-mcp-test-executable".into(),
        args: vec![],
        cwd: ctx.workspace.clone(),
        timeout: 1.0,
        plugin: "local:test".into(),
        version: "local".into(),
        digest: util::hash(b"{}"),
        source,
        transport: "stdio".into(),
        url: String::new(),
        env: std::collections::HashMap::new(),
        headers: std::collections::HashMap::new(),
        local: true,
        configuration_error: String::new(),
    };
    let registry = McpRegistry::prepare(vec![server.clone(), server]).await;
    assert_eq!(registry.statuses.len(), 1);
    assert!(registry.statuses[0]["error"].is_string());
    assert!(registry.tools.is_empty());
}
#[test]
fn report_preserves_released_html_layout_and_diff_colours() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../tests/agent/fixtures/report.json")).unwrap();
    let events: Vec<crate::models::Event> =
        serde_json::from_value(fixture["events"].clone()).unwrap();
    let messages: Vec<crate::models::Message> =
        serde_json::from_value(fixture["messages"].clone()).unwrap();
    let full = serde_json::from_value(fixture["full_outputs"].clone()).unwrap();
    let actual = crate::report::render(&fixture["summary"], &events, &messages, &full);
    let expected = include_str!("../tests/agent/fixtures/report.html");
    assert_eq!(actual.replace("\r\n", "\n"), expected.replace("\r\n", "\n"));
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

#[cfg(unix)]
#[test]
fn session_lock_release_does_not_wait_for_a_forked_child() {
    let (_dir, ctx) = fixture();
    let guard = ctx.store.activation(&ctx.session).unwrap();
    let mut pipe = [0; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // Only async-signal-safe syscalls are allowed in the forked child.
        unsafe {
            libc::close(pipe[1]);
            let mut signal = 0_u8;
            libc::read(pipe[0], (&mut signal as *mut u8).cast(), 1);
            libc::_exit(0);
        }
    }
    assert!(pid > 0, "fork failed");
    unsafe { libc::close(pipe[0]) };
    drop(guard);
    let reacquired = ctx.store.activation(&ctx.session);
    // Reap the child before asserting, even if the old implementation failed.
    unsafe {
        libc::write(pipe[1], b"x".as_ptr().cast(), 1);
        libc::close(pipe[1]);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
    assert!(reacquired.is_ok(), "session lock outlived its owner");
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

#[test]
fn context_estimate_matches_serialized_unicode_and_escapes() {
    fn chars(value: &serde_json::Value) -> u64 {
        match value {
            serde_json::Value::Array(a) => {
                2 + a.iter().map(chars).sum::<u64>() + a.len().saturating_sub(1) as u64 * 2
            }
            serde_json::Value::Object(o) => {
                2 + o
                    .iter()
                    .map(|(k, v)| chars(&json!(k)) + 2 + chars(v))
                    .sum::<u64>()
                    + o.len().saturating_sub(1) as u64 * 2
            }
            _ => value.to_string().chars().count() as u64,
        }
    }
    let text = (0..=0x1f)
        .map(|n| char::from_u32(n).unwrap())
        .collect::<String>()
        + "中文😀\"\\/\u{85}\u{2028}";
    let input = json!({text.clone():[text,null,true,false,-12,0,1.25,{},[],{"nested":"value"}]});
    let messages = vec![Message {
        role: "assistant".into(),
        content: vec![Block::ToolUse {
            id: "one".into(),
            name: "test".into(),
            input: input.clone(),
        }],
    }];
    let mut specs = tools::builtins().clone();
    specs.push(crate::models::ToolSpec {
        name: text.clone(),
        description: text,
        input_schema: input.clone(),
        requires_approval: false,
    });
    let expected = context::tokens("test")
        + (chars(&input) / 3).max(1)
        + specs
            .iter()
            .map(|s| (chars(&serde_json::to_value(s).unwrap()) / 3).max(1))
            .sum::<u64>();
    assert_eq!(context::estimate("", &messages, &specs), expected);
}

#[test]
fn replacement_and_counter_updates_preserve_atomic_checkpoints() {
    let (_dir, ctx) = fixture();
    let messages = (0..2000)
        .map(|i| Message::user(format!("message {i} 中文")))
        .collect::<Vec<_>>();
    ctx.store.replace_messages(&ctx.session, &messages).unwrap();
    ctx.store
        .append(&ctx.session, &Message::user("last"))
        .unwrap();
    assert_eq!(ctx.store.messages(&ctx.session).unwrap().len(), 2001);
    ctx.store
        .replace_messages(&ctx.session, &messages[..3])
        .unwrap();
    assert_eq!(ctx.store.messages(&ctx.session).unwrap(), messages[..3]);
    assert!(
        ctx.store
            .update(&ctx.session, &json!({"rounds":10,"invalid":1}))
            .is_err()
    );
    assert_eq!(ctx.store.require(&ctx.session).unwrap()["rounds"], 0);
    assert!(
        ctx.store
            .checkpoint(
                &ctx.session,
                "invalid",
                &json!({}),
                Some(&Message::user("rollback")),
                None,
                Some(&json!({"rounds":10,"model":"forbidden"}))
            )
            .is_err()
    );
    assert_eq!(ctx.store.messages(&ctx.session).unwrap(), messages[..3]);
    assert!(ctx.store.events(&ctx.session, -1).unwrap().is_empty());
    ctx.store.replace_messages(&ctx.session, &[]).unwrap();
    ctx.store
        .append(&ctx.session, &Message::user("new first"))
        .unwrap();
    assert_eq!(
        ctx.store.messages(&ctx.session).unwrap(),
        vec![Message::user("new first")]
    );
}

#[test]
fn statistics_projection_preserves_usage_and_advances_over_ignored_events() {
    let (_dir, ctx) = fixture();
    let data = json!({"text":"x".repeat(100000),"reasoning":"ignored","usage":{"output_tokens":42,"available":false},"request_seconds":2.5,"generation_seconds":1.5,"first_token_seconds":0.3});
    ctx.store
        .event(&ctx.session, "assistant_message", &data)
        .unwrap();
    ctx.store
        .event(
            &ctx.session,
            "tool_call_start",
            &json!({"arguments":"large"}),
        )
        .unwrap();
    ctx.store
        .event(
            &ctx.session,
            "tool_call_result",
            &json!({"output_detail":"x".repeat(100000)}),
        )
        .unwrap();
    let (until, events) = ctx.store.statistics_events(&ctx.session, -1).unwrap();
    assert_eq!(until, 2);
    assert_eq!(events.len(), 2);
    for key in [
        "usage",
        "request_seconds",
        "generation_seconds",
        "first_token_seconds",
    ] {
        assert_eq!(events[0].data[key], data[key]);
    }
    assert!(events[0].data.get("text").is_none());
    assert_eq!(events[1].data, json!({}));
    assert!(
        ctx.store
            .statistics_events(&ctx.session, until)
            .unwrap()
            .1
            .is_empty()
    );
    ctx.store
        .event(&ctx.session, "assistant_message", &json!({}))
        .unwrap();
    let (next, events) = ctx.store.statistics_events(&ctx.session, until).unwrap();
    assert_eq!(next, 3);
    assert_eq!(events.len(), 1);
    assert!(events[0].data["usage"].is_null());
}

#[test]
fn provider_payload_ownership_keeps_both_wire_formats_exact() {
    let (_dir, ctx) = fixture();
    let mut provider = crate::provider::Provider::new(
        "fake",
        "off",
        &Configuration {
            path: ctx.workspace.join("config.json"),
        },
    )
    .unwrap();
    provider.max_output = 32;
    let messages = vec![
        Message::user("hello 中文"),
        Message {
            role: "assistant".into(),
            content: vec![
                Block::text("answer"),
                Block::ToolUse {
                    id: "one".into(),
                    name: "read".into(),
                    input: json!({"path":"file"}),
                },
            ],
        },
        Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: "one".into(),
                content: "result".into(),
                is_error: false,
                images: vec![],
            }],
        },
    ];
    let openai = provider
        .payload("system", &messages, &[], &ctx.attachments)
        .unwrap();
    assert_eq!(
        openai,
        json!({"model":"fake","messages":[{"role":"system","content":"system"},{"role":"user","content":"hello 中文"},{"role":"assistant","content":"answer","tool_calls":[{"id":"one","type":"function","function":{"name":"read","arguments":"{\"path\":\"file\"}"}}]},{"role":"tool","tool_call_id":"one","content":"result"}],"stream":true,"max_tokens":32,"stream_options":{"include_usage":true}})
    );
    provider.style = "anthropic".into();
    let anthropic = provider
        .payload("system", &messages, &[], &ctx.attachments)
        .unwrap();
    assert_eq!(
        anthropic,
        json!({"model":"fake","messages":[{"role":"user","content":[{"type":"text","text":"hello 中文"}]},{"role":"assistant","content":[{"type":"text","text":"answer"},{"type":"tool_use","id":"one","name":"read","input":{"path":"file"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"one","content":"result","is_error":false}]}],"stream":true,"max_tokens":32,"system":"system"})
    );
}

#[tokio::test]
async fn empty_file_is_a_successful_read() {
    let (_dir, ctx) = fixture();
    fs::write(ctx.workspace.join("empty.txt"), "").unwrap();
    let result = tools::execute("read", &json!({"path":"empty.txt"}), &ctx).await;
    assert!(result.success, "{:?}", result.error);
    assert!(result.output.contains("empty"));
    let beyond = tools::execute("read", &json!({"path":"empty.txt","offset":2}), &ctx).await;
    assert!(!beyond.success);
}

#[tokio::test]
async fn multiline_edits_preserve_crlf_for_small_and_large_files() {
    let (_dir, ctx) = fixture();
    for (name, padding) in [("small.txt", 0), ("large.txt", 300_000)] {
        let before = "x".repeat(padding) + "one\r\ntwo\r\n";
        fs::write(ctx.workspace.join(name), before).unwrap();
        let result = tools::execute(
            "edit",
            &json!({"path":name,"old_text":"one\ntwo\n","new_text":"one\nchanged\n"}),
            &ctx,
        )
        .await;
        assert!(result.success, "{:?}", result.error);
        assert!(
            fs::read_to_string(ctx.workspace.join(name))
                .unwrap()
                .ends_with("one\r\nchanged\r\n")
        );
    }
}

#[tokio::test]
async fn scoped_search_results_always_use_workspace_relative_paths() {
    let (_dir, ctx) = fixture();
    fs::create_dir(ctx.workspace.join("src")).unwrap();
    fs::write(ctx.workspace.join("src/file.txt"), "needle").unwrap();
    for path in ["src", "src/file.txt"] {
        let result = tools::execute("grep", &json!({"path":path,"pattern":"needle"}), &ctx).await;
        assert!(result.success);
        assert!(
            result.output.starts_with("src/file.txt:1:"),
            "{}",
            result.output
        );
    }
}

#[test]
fn manual_compaction_keeps_original_constraints_and_recent_messages() {
    let (_dir, ctx) = fixture();
    let original = "Keep this entire original request. ".repeat(40) + "FINAL_CONSTRAINT";
    let mut messages = vec![
        Message::user(&original),
        Message {
            role: "assistant".into(),
            content: vec![Block::text("first reply")],
        },
        Message::user("second task"),
        Message {
            role: "assistant".into(),
            content: vec![Block::text("second reply\n".repeat(200))],
        },
        Message::user("latest task"),
    ];
    assert!(
        context::compact(
            &ctx.store,
            &ctx.session,
            "",
            &[],
            &mut messages,
            200_000,
            true
        )
        .unwrap()
        .is_some()
    );
    assert_eq!(messages.first().unwrap().text(), original);
    assert_eq!(messages.last().unwrap().text(), "latest task");
    assert!(messages.iter().any(|m| m.text().contains("[artifact:")));
}

#[test]
fn plugin_fingerprints_distinguish_filename_content_boundary_collisions() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    fs::write(first.join("a"), "bc").unwrap();
    fs::write(second.join("ab"), "c").unwrap();
    assert_ne!(
        crate::extensions::fingerprint(&first).unwrap(),
        crate::extensions::fingerprint(&second).unwrap()
    );
}

#[test]
fn reimporting_identical_media_with_a_new_name_keeps_old_messages_readable() {
    let (_dir, ctx) = fixture();
    let bytes = b"\x89PNG\r\n\x1a\n";
    let first = ctx
        .attachments
        .import_bytes(bytes, "first.png", "image/png")
        .unwrap();
    let second = ctx
        .attachments
        .import_bytes(bytes, "renamed.png", "image/png")
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(
        ctx.attachments.encoded(&first).unwrap(),
        ctx.attachments.encoded(&second).unwrap()
    );
}

#[tokio::test]
async fn ordinary_command_newlines_do_not_create_an_artifact() {
    let (_dir, ctx) = fixture();
    let command = if cfg!(windows) {
        "Write-Output 'hello'"
    } else {
        "printf 'hello\\n'"
    };
    let result = tools::run_command(command, &ctx.workspace, 10.0, &ctx)
        .await
        .unwrap();
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.output, "hello");
    assert!(
        ctx.store
            .with(
                |c| Ok(c.query_row("SELECT COUNT(*) FROM artifacts", [], |r| r.get::<_, i64>(0))?)
            )
            .unwrap()
            == 0
    );
}

#[tokio::test]
async fn shell_preserves_quoted_command_text() {
    let (_dir, ctx) = fixture();
    let result = tools::run_command("echo '1>/dev/null'", &ctx.workspace, 10.0, &ctx)
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(result.output, "1>/dev/null");
}

#[tokio::test]
async fn failed_acceptance_invalidates_previous_passing_evidence() {
    let (_dir, ctx) = fixture();
    let spec: crate::goals::Spec = serde_json::from_value(
        json!({"items":[{"id":"file","type":"artifact","path":"required.txt"}]}),
    )
    .unwrap();
    let mut goal = crate::goals::Goal::new(spec, &ctx.workspace).unwrap();
    fs::write(ctx.workspace.join("required.txt"), "ok").unwrap();
    assert_eq!(goal.check(&ctx).await.unwrap()["passed"], true);
    assert!(goal.evidence.is_some());
    fs::remove_file(ctx.workspace.join("required.txt")).unwrap();
    assert_eq!(goal.check(&ctx).await.unwrap()["passed"], false);
    assert!(goal.evidence.is_none());
}

#[tokio::test]
async fn protected_paths_cannot_pass_by_becoming_directories() {
    let (_dir, ctx) = fixture();
    let spec: crate::goals::Spec = serde_json::from_value(
        json!({"items":[{"id":"protected","type":"protected","path":"protected.txt"}]}),
    )
    .unwrap();
    let mut goal = crate::goals::Goal::new(spec.clone(), &ctx.workspace).unwrap();
    assert_eq!(goal.check(&ctx).await.unwrap()["passed"], true);
    fs::create_dir(ctx.workspace.join("protected.txt")).unwrap();
    assert_eq!(goal.check(&ctx).await.unwrap()["passed"], false);
    let mut directory = crate::goals::Goal::new(spec, &ctx.workspace).unwrap();
    assert_eq!(directory.check(&ctx).await.unwrap()["passed"], false);
}

#[tokio::test]
async fn cancelling_a_live_descendant_releases_its_working_directory() {
    let (_dir, ctx) = fixture();
    let cwd = ctx.workspace.join("command-cwd");
    fs::create_dir(&cwd).unwrap();
    let child_ctx = ctx.clone();
    let child_cwd = cwd.clone();
    let task = tokio::spawn(async move {
        tools::run_command("node -e \"require('fs').writeFileSync('child.pid',String(process.pid));setInterval(()=>{},1000)\"", &child_cwd, 20.0, &child_ctx).await
    });
    for _ in 0..500 {
        if cwd.join("child.pid").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(cwd.join("child.pid").exists(), "command did not start");
    ctx.cancel.cancel();
    assert!(task.await.unwrap().is_err());
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn running_sessions_cannot_be_deleted_from_a_second_store() {
    let (_dir, ctx) = fixture();
    let other = Store::open(&ctx.store.path).unwrap();
    let lock = ctx.store.activation(&ctx.session).unwrap();
    assert!(other.delete(&ctx.session).is_err());
    assert!(ctx.store.require(&ctx.session).is_ok());
    drop(lock);
    other.delete(&ctx.session).unwrap();
    assert!(ctx.store.require(&ctx.session).is_err());
}

// Runtime harness: a real Runtime driven by the scripted fake provider, with
// every bridge event captured. Main-loop regressions build on this instead of
// a network endpoint.
struct Harness {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    runtime: crate::runtime::Runtime,
    hooks: crate::runtime::Hooks,
    events: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}
impl Harness {
    async fn new(script: serde_json::Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = util::canonical(dir.path()).unwrap();
        let workspace = root.join("repo");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("README.md"), "harness repository\n").unwrap();
        let store = Store::open(root.join("sessions.db")).unwrap();
        let configuration = Configuration {
            path: root.join("config.json"),
        };
        let settings = crate::runtime::Settings {
            model: "fake".into(),
            effort: "off".into(),
            mode: "bypass".into(),
            budget: crate::models::Budget::default(),
            acceptance: None,
            always_allow: Default::default(),
            pending_model: false,
            registry_dirty: false,
        };
        let mut runtime =
            crate::runtime::Runtime::new(&workspace, "fake", None, store, configuration, &settings)
                .await
                .unwrap();
        runtime.set_script(script).unwrap();
        let events = Arc::new(std::sync::Mutex::new(vec![]));
        let captured = events.clone();
        let hooks = crate::runtime::Hooks {
            settings: Arc::new(std::sync::Mutex::new(settings)),
            snapshot: Default::default(),
            approvals: Default::default(),
            approval_sequence: Default::default(),
            sink: Arc::new(move |value| captured.lock().unwrap().push(value)),
            cancel: CancellationToken::new(),
            shared: Default::default(),
        };
        Self {
            _dir: dir,
            root,
            runtime,
            hooks,
            events,
        }
    }
    async fn run(&mut self, text: Option<&str>) -> anyhow::Result<crate::models::RunResult> {
        self.hooks.cancel = CancellationToken::new();
        self.runtime
            .run(text.map(str::to_string), vec![], self.hooks.clone())
            .await
    }
    fn event_types(&self) -> Vec<String> {
        self.runtime
            .store
            .events(&self.runtime.session, -1)
            .unwrap()
            .into_iter()
            .map(|e| e.r#type)
            .collect()
    }
    /// The exact Anthropic `messages` array the next request would send.
    fn anthropic_messages(&self) -> Vec<serde_json::Value> {
        let mut provider = self.runtime.provider.clone();
        provider.style = "anthropic".into();
        let attachments = Attachments {
            root: self.root.join("attachments"),
        };
        provider
            .payload("system", &self.runtime.messages, &[], &attachments)
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .clone()
    }
}
fn assert_strict_anthropic_roles(messages: &[serde_json::Value]) {
    assert_eq!(messages.first().map(|m| &m["role"]), Some(&json!("user")));
    for pair in messages.windows(2) {
        assert_ne!(pair[0]["role"], pair[1]["role"], "{messages:#?}");
    }
    for message in messages.iter().filter(|m| m["role"] == "user") {
        let content = message["content"].as_array().unwrap();
        let first_other = content
            .iter()
            .position(|b| b["type"] != "tool_result")
            .unwrap_or(content.len());
        assert!(
            content[first_other..]
                .iter()
                .all(|b| b["type"] != "tool_result"),
            "tool_result blocks must lead the user message: {message:#}"
        );
    }
}

#[tokio::test]
async fn harness_fake_provider_scripts_errors_tool_calls_and_usage() {
    let mut h = Harness::new(json!({"turns":[
        {"error":"provider HTTP 503"},
        {"tool_calls":[{"name":"read","arguments":{"path":"README.md"}}],"input_tokens":7,"output_tokens":3},
        {"text":"done"}
    ]}))
    .await;
    let result = h.run(Some("inspect")).await.unwrap();
    assert_eq!(result.exit_reason, "completed");
    let types = h.event_types();
    assert_eq!(types.iter().filter(|t| *t == "provider_retry").count(), 1);
    assert!(types.contains(&"tool_call_result".to_string()));
    assert_eq!(result.total_usage.input_tokens, 107);
    assert_strict_anthropic_roles(&h.anthropic_messages());
    let sink = h.events.lock().unwrap();
    assert!(
        sink.iter()
            .any(|e| e["event"] == "text_delta" && e["text"] == "done")
    );
}

// R40: every runtime path that appends a user notice right after a user
// tool_result must still produce a strictly alternating Anthropic payload.
#[tokio::test]
async fn anthropic_roles_alternate_after_truncated_response() {
    let mut h = Harness::new(json!({"turns":[
        {"text":"partial","stop_reason":"max_tokens","tool_calls":[{"name":"write","arguments":{"path":"x.txt","content":"no"}}]},
        {"text":"done"}
    ]}))
    .await;
    assert_eq!(h.run(Some("task")).await.unwrap().exit_reason, "completed");
    let roles = h
        .runtime
        .messages
        .iter()
        .map(|m| m.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, ["user", "assistant", "user", "user", "assistant"]);
    let messages = h.anthropic_messages();
    assert_strict_anthropic_roles(&messages);
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][1]["type"], "text");
}

#[tokio::test]
async fn anthropic_roles_alternate_after_stall_notice() {
    let call = json!({"tool_calls":[{"name":"read","arguments":{"path":"README.md"}}]});
    let mut h = Harness::new(json!({"turns":[call,call,call,call,call,call]})).await;
    assert_eq!(h.run(Some("task")).await.unwrap().exit_reason, "stalled");
    assert_eq!(
        h.runtime
            .messages
            .iter()
            .rev()
            .take(2)
            .map(|m| m.role.as_str())
            .collect::<Vec<_>>(),
        ["user", "user"]
    );
    assert_strict_anthropic_roles(&h.anthropic_messages());
}

#[tokio::test]
async fn anthropic_roles_alternate_after_background_completion_notice() {
    let mut h = Harness::new(json!({"turns":[
        {"tool_calls":[
            {"name":"bash","arguments":{"command":"node -e \"console.log(42)\"","background":true}},
            {"name":"bash","arguments":{"command":"node -e \"setTimeout(()=>{},1500)\""}}
        ]},
        {"text":"done"}
    ]}))
    .await;
    assert_eq!(h.run(Some("task")).await.unwrap().exit_reason, "completed");
    assert!(
        h.runtime
            .messages
            .iter()
            .any(|m| m.role == "user" && m.text().starts_with("后台任务已完成"))
    );
    assert_strict_anthropic_roles(&h.anthropic_messages());
}

#[tokio::test]
async fn anthropic_roles_alternate_after_recovery_backfill_and_lost_job() {
    let mut h = Harness::new(json!({"turns":[{"text":"recovered"}]})).await;
    let session = h.runtime.session.clone();
    for message in [
        Message::user("task"),
        Message {
            role: "assistant".into(),
            content: vec![Block::ToolUse {
                id: "interrupted".into(),
                name: "write".into(),
                input: json!({"path":"x.txt","content":"?"}),
            }],
        },
    ] {
        h.runtime.store.append(&session, &message).unwrap();
        h.runtime.messages.push(message);
    }
    h.runtime
        .store
        .event(
            &session,
            "background_job_started",
            &json!({"job_id":"bg_lost","command":"sleep"}),
        )
        .unwrap();
    h.runtime.pending = true;
    assert_eq!(h.run(None).await.unwrap().exit_reason, "completed");
    let types = h.event_types();
    assert!(types.contains(&"side_effect_unknown".to_string()));
    assert!(types.contains(&"background_job_lost".to_string()));
    let messages = h.anthropic_messages();
    assert_strict_anthropic_roles(&messages);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "interrupted");
}

#[test]
fn anthropic_normalization_merges_and_orders_blocks_without_touching_history() {
    let (_dir, ctx) = fixture();
    let mut provider = crate::provider::Provider::new(
        "fake",
        "off",
        &Configuration {
            path: ctx.workspace.join("config.json"),
        },
    )
    .unwrap();
    provider.style = "anthropic".into();
    let result = |id: &str| Block::ToolResult {
        tool_use_id: id.into(),
        content: "ok".into(),
        is_error: false,
        images: vec![],
    };
    let messages = vec![
        Message::user("first"),
        Message::user("second"),
        Message {
            role: "assistant".into(),
            content: vec![
                Block::ToolUse {
                    id: "a".into(),
                    name: "read".into(),
                    input: json!({}),
                },
                Block::ToolUse {
                    id: "b".into(),
                    name: "read".into(),
                    input: json!({}),
                },
            ],
        },
        Message {
            role: "user".into(),
            content: vec![Block::text("note before"), result("a")],
        },
        Message {
            role: "user".into(),
            content: vec![result("b"), Block::text("note after")],
        },
    ];
    let original = messages.clone();
    let payload = provider
        .payload("system", &messages, &[], &ctx.attachments)
        .unwrap();
    assert_eq!(messages, original);
    assert_eq!(
        payload["messages"],
        json!([
            {"role":"user","content":[{"type":"text","text":"first"},{"type":"text","text":"second"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"a","name":"read","input":{}},{"type":"tool_use","id":"b","name":"read","input":{}}]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"a","content":"ok","is_error":false},
                {"type":"tool_result","tool_use_id":"b","content":"ok","is_error":false},
                {"type":"text","text":"note before"},
                {"type":"text","text":"note after"}
            ]}
        ])
    );
    provider.style = "openai".into();
    let openai = provider
        .payload("system", &messages, &[], &ctx.attachments)
        .unwrap();
    assert_eq!(
        openai["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "user")
            .count(),
        4
    );
}

// R41 / B16: run teardown is best-effort. Storage failures and panics must not
// skip cancelling background jobs, closing MCP, clearing approvals, writing
// session_end or resetting the running flag.
fn inject_storage_failure(h: &Harness, event_type: &str) {
    rusqlite::Connection::open(&h.runtime.store.path)
        .unwrap()
        .execute_batch(&format!(
            "CREATE TRIGGER inject_failure BEFORE INSERT ON events WHEN NEW.type='{event_type}' BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;"
        ))
        .unwrap();
}
fn background_then(last: serde_json::Value) -> serde_json::Value {
    json!({"turns":[
        {"tool_calls":[
            // Bounded lifetime: a leaked job must not outlive the test run.
            {"name":"bash","arguments":{"command":"node -e \"setTimeout(()=>{},30000)\"","background":true}}
        ]},
        last
    ]})
}
fn assert_torn_down(h: &Harness) {
    let snapshot = h.hooks.snapshot.lock().unwrap();
    assert!(!snapshot.running);
    assert_ne!(snapshot.phase, "model");
    drop(snapshot);
    assert!(h.hooks.approvals.lock().unwrap().is_empty());
    assert!(h.runtime.mcp.statuses.is_empty());
}

#[tokio::test]
async fn teardown_continues_after_a_failed_background_job_event() {
    let mut h = Harness::new(background_then(json!({"text":"done"}))).await;
    inject_storage_failure(&h, "background_job_lost");
    let error = h.run(Some("task")).await.unwrap_err().to_string();
    assert!(error.contains("injected storage failure"), "{error}");
    assert_torn_down(&h);
    assert!(h.event_types().contains(&"session_end".to_string()));
}

#[tokio::test]
async fn teardown_reports_a_failed_session_end_and_resets_running() {
    let mut h = Harness::new(background_then(json!({"text":"done"}))).await;
    inject_storage_failure(&h, "session_end");
    let error = h.run(Some("task")).await.unwrap_err().to_string();
    assert!(error.contains("injected storage failure"), "{error}");
    assert_torn_down(&h);
    let types = h.event_types();
    assert!(types.contains(&"background_job_lost".to_string()));
    assert!(!types.contains(&"session_end".to_string()));
}

#[tokio::test]
async fn a_panicking_run_is_torn_down_and_reported() {
    let mut h = Harness::new(background_then(json!({"panic":"boom"}))).await;
    let error = h.run(Some("task")).await.unwrap_err().to_string();
    assert!(error.contains("panic") && error.contains("boom"), "{error}");
    assert_torn_down(&h);
    let events = h.runtime.store.events(&h.runtime.session, -1).unwrap();
    assert!(events.iter().any(|e| e.r#type == "background_job_lost"));
    let end = events.iter().find(|e| e.r#type == "session_end").unwrap();
    assert_eq!(end.data["exit_reason"], "internal_error");
    assert!(h.runtime.pending);
}
