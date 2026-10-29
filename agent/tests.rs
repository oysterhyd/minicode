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
        files: Default::default(),
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

fn t9_reorder_and_extend_artifacts(store: &Store) {
    store.with(|c| {c.execute_batch("CREATE TABLE migrated_artifacts(created_at TEXT NOT NULL,artifact_id TEXT NOT NULL,kind TEXT NOT NULL,session_id TEXT NOT NULL,relpath TEXT NOT NULL,migration_marker TEXT NOT NULL DEFAULT 'migrated',PRIMARY KEY(session_id,artifact_id));
        INSERT INTO migrated_artifacts(created_at,artifact_id,kind,session_id,relpath) SELECT created_at,artifact_id,kind,session_id,relpath FROM artifacts;
        DROP TABLE artifacts; ALTER TABLE migrated_artifacts RENAME TO artifacts;")?;Ok(())}).unwrap();
}
#[test]
fn t9_spill_inserts_by_column_name_after_schema_migration() {
    let (_dir, ctx) = fixture();
    let old = ctx
        .store
        .spill(&ctx.session, "history", "old content")
        .unwrap();
    t9_reorder_and_extend_artifacts(&ctx.store);
    let new = ctx
        .store
        .spill(&ctx.session, "tool_output", "new content")
        .unwrap();
    assert_eq!(
        ctx.store
            .artifact_page(&ctx.session, &old, 0, 4000)
            .unwrap()["text"],
        "old content"
    );
    assert_eq!(
        ctx.store
            .artifact_page(&ctx.session, &new, 0, 4000)
            .unwrap()["text"],
        "new content"
    );
    ctx.store
        .with(|c| {
            let marker: String = c.query_row(
                "SELECT migration_marker FROM artifacts WHERE session_id=? AND artifact_id=?",
                rusqlite::params![ctx.session, new],
                |row| row.get(0),
            )?;
            assert_eq!(marker, "migrated");
            Ok(())
        })
        .unwrap();
}
#[tokio::test]
async fn t9_command_capture_inserts_by_column_name_after_schema_migration() {
    let (_dir, ctx) = fixture();
    t9_reorder_and_extend_artifacts(&ctx.store);
    let result = tools::run_command(
        "node -e \"process.stdout.write('x'.repeat(60000))\"",
        &ctx.workspace,
        10.0,
        &ctx,
    )
    .await
    .unwrap();
    assert!(result.success, "{:?}", result.error);
    assert!(result.output.contains("[artifact:"));
    ctx.store.with(|c| {let count:i64=c.query_row("SELECT COUNT(*) FROM artifacts WHERE kind='command_output' AND migration_marker='migrated'",[],|row|row.get(0))?;assert_eq!(count,1);Ok(())}).unwrap();
}

async fn t8_same_metadata_edit_case(padding: usize) {
    let (_dir, ctx) = fixture();
    let path = ctx.workspace.join("same-metadata.txt");
    let before = format!("header old\ntarget\n{}", "x".repeat(padding));
    fs::write(&path, &before).unwrap();
    assert!(
        tools::execute("read", &json!({"path":"same-metadata.txt"}), &ctx)
            .await
            .success
    );
    let modified = path.metadata().unwrap().modified().unwrap();
    let external = before.replacen("old", "new", 1);
    fs::write(&path, &external).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
    assert_eq!(before.len(), external.len());
    let out = tools::execute(
        "edit",
        &json!({"path":"same-metadata.txt","old_text":"target","new_text":"changed"}),
        &ctx,
    )
    .await;
    assert!(
        !out.success,
        "same-size same-mtime external edit was missed"
    );
    assert!(out.error.unwrap().contains("外部修改"));
    assert_eq!(fs::read_to_string(&path).unwrap(), external);
    assert!(
        tools::execute("read", &json!({"path":"same-metadata.txt"}), &ctx)
            .await
            .success
    );
    let refreshed = tools::execute(
        "edit",
        &json!({"path":"same-metadata.txt","old_text":"target","new_text":"changed"}),
        &ctx,
    )
    .await;
    assert!(refreshed.success, "{:?}", refreshed.error);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        external.replacen("target", "changed", 1)
    );
}
#[tokio::test]
async fn t8_small_edits_check_content_even_when_metadata_is_unchanged() {
    t8_same_metadata_edit_case(0).await;
}
#[tokio::test]
async fn t8_large_edits_check_content_even_when_metadata_is_unchanged() {
    t8_same_metadata_edit_case(400000).await;
}

#[tokio::test]
async fn t5_artifact_default_matches_schema_and_supports_lines_search_tail() {
    let (_dir, ctx) = fixture();
    let artifact = ctx
        .store
        .spill(
            &ctx.session,
            "command_output",
            &format!("one\n二\nneedle three\nfour\n{}", "tail\n".repeat(1200)),
        )
        .unwrap();
    let default = tools::execute("read_artifact", &json!({"artifact_id":artifact}), &ctx).await;
    assert!(default.success);
    assert_eq!(
        default
            .output
            .split("\n...[更多内容")
            .next()
            .unwrap()
            .chars()
            .count(),
        4000
    );
    for (extra, expected) in [
        (
            json!({"start_line":2,"end_line":3}),
            "2\t二\n3\tneedle three",
        ),
        (json!({"pattern":"needle"}), "3\tneedle three"),
        (json!({"tail_lines":2}), "1203\ttail\n1204\ttail"),
        (
            json!({"pattern":"(?i)NEEDLE","start_line":2}),
            "3\tneedle three",
        ),
    ] {
        let mut args = extra;
        args["artifact_id"] = json!(artifact);
        let result = tools::execute("read_artifact", &args, &ctx).await;
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.output, expected);
    }
    let invalid = tools::execute(
        "read_artifact",
        &json!({"artifact_id":artifact,"start_line":3,"end_line":1}),
        &ctx,
    )
    .await;
    assert!(!invalid.success);
}

#[tokio::test]
async fn t5_artifact_line_views_are_bounded_and_keep_session_isolation() {
    let (_dir, ctx) = fixture();
    let artifact = ctx
        .store
        .spill(
            &ctx.session,
            "command_output",
            &format!("{}needle\nother\n", "x".repeat(1100000)),
        )
        .unwrap();
    let large = tools::execute(
        "read_artifact",
        &json!({"artifact_id":artifact,"pattern":"needle"}),
        &ctx,
    )
    .await;
    assert!(!large.success);
    assert!(large.error.unwrap().contains("use character pagination"));
    let later = tools::execute(
        "read_artifact",
        &json!({"artifact_id":artifact,"start_line":2}),
        &ctx,
    )
    .await;
    assert!(later.success, "{:?}", later.error);
    assert_eq!(later.output, "2\tother");
    let chars = tools::execute(
        "read_artifact",
        &json!({"artifact_id":artifact,"limit":12}),
        &ctx,
    )
    .await;
    assert!(chars.success);
    assert!(chars.output.starts_with("xxxxxxxxxxxx\n"));
    let other = ctx
        .store
        .create(&ctx.workspace.to_string_lossy(), "fake", "fake")
        .unwrap();
    let foreign = ctx
        .store
        .spill(&other, "command_output", "private")
        .unwrap();
    let denied = tools::execute(
        "read_artifact",
        &json!({"artifact_id":foreign,"tail_lines":1}),
        &ctx,
    )
    .await;
    assert!(!denied.success);
    let short = ctx
        .store
        .spill(
            &ctx.session,
            "command_output",
            &format!("{}needle\n", "中".repeat(200)),
        )
        .unwrap();
    let tail = tools::execute(
        "read_artifact",
        &json!({"artifact_id":short,"tail_lines":1,"pattern":"needle$","limit":20}),
        &ctx,
    )
    .await;
    assert!(tail.success);
    assert!(tail.output.contains("行已截断"));
}

#[tokio::test]
async fn t4_edits_normalize_unicode_whitespace_and_relative_indentation() {
    let (_dir, ctx) = fixture();
    let path = ctx.workspace.join("normalize.txt");
    fs::write(&path, "    if (ok) {\r\n      log(“café”);  \r\n    }\r\n").unwrap();
    assert!(
        tools::execute("read", &json!({"path":"normalize.txt"}), &ctx)
            .await
            .success
    );
    let out=tools::execute("edit",&json!({"path":"normalize.txt","old_text":"if (ok) {\n  log(\"cafe\u{301}\");\n}","new_text":"if (ok) {\n  log(\"changed\");\n}"}),&ctx).await;
    assert!(out.success, "{:?}", out.error);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "    if (ok) {\r\n      log(\"changed\");\r\n    }\r\n"
    );
}

#[tokio::test]
async fn t4_batch_edits_are_atomic_and_use_original_offsets() {
    let (_dir, ctx) = fixture();
    let path = ctx.workspace.join("batch.txt");
    for (edits, expected, success) in [
        (
            json!([{"old_text":"one","new_text":"two"},{"old_text":"two","new_text":"three"}]),
            "two three",
            true,
        ),
        (
            json!([{"old_text":"one","new_text":"changed"},{"old_text":"missing","new_text":"no"}]),
            "one two",
            false,
        ),
        (
            json!([{"old_text":"one two","new_text":"all"},{"old_text":"two","new_text":"overlap"}]),
            "one two",
            false,
        ),
    ] {
        fs::write(&path, "one two").unwrap();
        assert!(
            tools::execute("read", &json!({"path":"batch.txt"}), &ctx)
                .await
                .success
        );
        let out = tools::execute("edit", &json!({"path":"batch.txt","edits":edits}), &ctx).await;
        assert_eq!(out.success, success, "{:?}", out.error);
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    }
}

#[tokio::test]
async fn t4_failed_edits_report_nearest_lines_and_reject_ambiguity() {
    let (_dir, ctx) = fixture();
    let path = ctx.workspace.join("candidate.txt");
    fs::write(&path, "heading\nconst amount = total + 1;\nfooter\n").unwrap();
    assert!(
        tools::execute("read", &json!({"path":"candidate.txt"}), &ctx)
            .await
            .success
    );
    let out = tools::execute(
        "edit",
        &json!({"path":"candidate.txt","old_text":"const amunt = total + 1;","new_text":"no"}),
        &ctx,
    )
    .await;
    assert!(!out.success);
    assert!(out.error.unwrap().contains("2\tconst amount = total + 1;"));
    fs::write(&path, "log(“x”);\nlog(\"x\");\n").unwrap();
    assert!(
        tools::execute("read", &json!({"path":"candidate.txt"}), &ctx)
            .await
            .success
    );
    let out = tools::execute(
        "edit",
        &json!({"path":"candidate.txt","old_text":"log(”x“);","new_text":"no"}),
        &ctx,
    )
    .await;
    assert!(!out.success);
    assert!(out.error.unwrap().contains("2 locations"));
}

#[tokio::test]
async fn t4_large_batch_and_exact_mode_keep_safety_boundaries() {
    let (_dir, ctx) = fixture();
    let path = ctx.workspace.join("large-normalized.txt");
    let before = format!("{}log(“café”);\ntail\n", "filler\n".repeat(40000));
    fs::write(&path, &before).unwrap();
    assert!(
        tools::execute("read", &json!({"path":"large-normalized.txt"}), &ctx)
            .await
            .success
    );
    let exact=tools::execute("edit",&json!({"path":"large-normalized.txt","old_text":"log(\"café\");","new_text":"changed","match_mode":"exact"}),&ctx).await;
    assert!(!exact.success);
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    let out=tools::execute("edit",&json!({"path":"large-normalized.txt","edits":[{"old_text":"log(\"cafe\u{301}\");","new_text":"changed"},{"old_text":"tail","new_text":"end"}]}),&ctx).await;
    assert!(out.success, "{:?}", out.error);
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .ends_with("changed\nend\n")
    );
    fs::write(&path, "é").unwrap();
    assert!(
        tools::execute("read", &json!({"path":"large-normalized.txt"}), &ctx)
            .await
            .success
    );
    let partial = tools::execute(
        "edit",
        &json!({"path":"large-normalized.txt","old_text":"e","new_text":"wrong"}),
        &ctx,
    )
    .await;
    assert!(!partial.success);
    assert_eq!(fs::read_to_string(&path).unwrap(), "é");
}

#[tokio::test]
async fn t2_grep_respects_ignore_rules_negation_and_subdirectory_searches() {
    let (_dir, ctx) = fixture();
    fs::write(
        ctx.workspace.join(".gitignore"),
        "target/\ndist/\nbuild/\n.next/\n*.log\n!keep.log\n",
    )
    .unwrap();
    fs::write(ctx.workspace.join(".ignore"), "private.txt\n").unwrap();
    for folder in ["target", "dist", "build", ".next", "node_modules", "src"] {
        fs::create_dir(ctx.workspace.join(folder)).unwrap();
    }
    for file in [
        "target/generated.txt",
        "dist/bundle.txt",
        "build/object.txt",
        ".next/cache.txt",
        "node_modules/pkg.txt",
        "private.txt",
        "src/hidden.log",
        "src/keep.log",
        "src/main.txt",
    ] {
        fs::write(ctx.workspace.join(file), "needle\n").unwrap();
    }
    fs::write(ctx.workspace.join("src/.gitignore"), "nested.txt\n").unwrap();
    fs::write(ctx.workspace.join("src/nested.txt"), "needle\n").unwrap();
    for path in [".", "src"] {
        let out = tools::execute("grep", &json!({"pattern":"needle","path":path}), &ctx).await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(out.output, "src/keep.log:1: needle\nsrc/main.txt:1: needle");
    }
    let explicit = tools::execute(
        "grep",
        &json!({"pattern":"needle","path":"src/hidden.log"}),
        &ctx,
    )
    .await;
    assert!(explicit.success);
    assert!(explicit.output.contains("src/hidden.log:1"));
    let all = tools::execute(
        "grep",
        &json!({"pattern":"needle","include_ignored":true}),
        &ctx,
    )
    .await;
    assert!(all.success);
    assert!(all.output.contains("target/generated.txt"));
    assert!(all.output.contains("private.txt"));
}

#[tokio::test]
async fn t2_grep_uses_a_non_backtracking_regex_engine() {
    let (_dir, ctx) = fixture();
    fs::write(
        ctx.workspace.join("input.txt"),
        format!("{}!\nneedle\n", "a".repeat(200_000)),
    )
    .unwrap();
    let out = tools::execute(
        "grep",
        &json!({"pattern":"^(a+)+$","path":"input.txt"}),
        &ctx,
    )
    .await;
    assert!(out.success, "{:?}", out.error);
    assert_eq!(out.output, "(no matches)");
    let unsupported = tools::execute(
        "grep",
        &json!({"pattern":"needle(?=$)","path":"input.txt"}),
        &ctx,
    )
    .await;
    assert!(!unsupported.success, "lookaround must fail explicitly");
}

#[tokio::test]
async fn t1_read_edit_write_preserve_encoding_and_bom() {
    let (_dir, ctx) = fixture();
    let utf16 = |text: &str, le: bool| -> Vec<u8> {
        text.encode_utf16()
            .flat_map(|v| if le { v.to_le_bytes() } else { v.to_be_bytes() })
            .collect()
    };
    let cases = [
        (
            [b"\xef\xbb\xbf".as_slice(), "中文\n".as_bytes()].concat(),
            [b"\xef\xbb\xbf".as_slice(), "世界\n".as_bytes()].concat(),
            [b"\xef\xbb\xbf".as_slice(), "中文\n".as_bytes()].concat(),
        ),
        (
            vec![0xd6, 0xd0, 0xce, 0xc4, 10],
            vec![0xca, 0xc0, 0xbd, 0xe7, 10],
            vec![0xd6, 0xd0, 0xce, 0xc4, 10],
        ),
        (
            [vec![255, 254], utf16("中文\n", true)].concat(),
            [vec![255, 254], utf16("世界\n", true)].concat(),
            [vec![255, 254], utf16("中文\n", true)].concat(),
        ),
        (
            [vec![254, 255], utf16("中文\n", false)].concat(),
            [vec![254, 255], utf16("世界\n", false)].concat(),
            [vec![254, 255], utf16("中文\n", false)].concat(),
        ),
    ];
    for (i, (before, edited, written)) in cases.into_iter().enumerate() {
        let path = format!("encoded-{i}.txt");
        fs::write(ctx.workspace.join(&path), before).unwrap();
        let read = tools::execute("read", &json!({"path":path}), &ctx).await;
        assert!(read.success, "{i}: {:?}", read.error);
        assert_eq!(read.output, "1\t中文");
        let edit = tools::execute(
            "edit",
            &json!({"path":path,"old_text":"中文","new_text":"世界"}),
            &ctx,
        )
        .await;
        assert!(edit.success, "{i}: {:?}", edit.error);
        assert_eq!(fs::read(ctx.workspace.join(&path)).unwrap(), edited);
        let write = tools::execute("write", &json!({"path":path,"content":"中文\n"}), &ctx).await;
        assert!(write.success, "{i}: {:?}", write.error);
        assert_eq!(fs::read(ctx.workspace.join(&path)).unwrap(), written);
    }
}

#[tokio::test]
async fn t1_reads_notebook_cells_and_text_outputs() {
    let (_dir, ctx) = fixture();
    fs::write(ctx.workspace.join("notes.ipynb"),serde_json::to_vec(&json!({"nbformat":4,"cells":[
        {"cell_type":"markdown","source":["# 标题\n","正文"]},
        {"cell_type":"code","source":"print('hello')","outputs":[{"output_type":"stream","text":["hello\n"]},{"output_type":"display_data","data":{"text/plain":["result"],"text/html":["ignored-html"]}}]}
    ]})).unwrap()).unwrap();
    let result = tools::execute("read", &json!({"path":"notes.ipynb"}), &ctx).await;
    assert!(result.success, "{:?}", result.error);
    assert!(result.output.contains("Cell 1 [markdown]"));
    assert!(result.output.contains("Cell 2 [code]"));
    assert!(result.output.contains("# 标题"));
    assert!(result.output.contains("hello"));
    assert!(!result.output.contains("ignored-html"));
}

#[tokio::test]
async fn t1_encoding_pagination_and_unrepresentable_edits_are_lossless() {
    let (_dir, ctx) = fixture();
    let path = ctx.workspace.join("gbk.txt");
    let original = [
        "ascii prefix\n".repeat(1000).into_bytes(),
        vec![0xd6, 0xd0, 0xce, 0xc4, 10],
    ]
    .concat();
    fs::write(&path, &original).unwrap();
    let page = tools::execute("read", &json!({"path":"gbk.txt","offset":1001}), &ctx).await;
    assert!(page.success, "{:?}", page.error);
    assert_eq!(page.output, "1001\t中文");
    let failed = tools::execute(
        "edit",
        &json!({"path":"gbk.txt","old_text":"中文","new_text":"😀"}),
        &ctx,
    )
    .await;
    assert!(!failed.success);
    assert_eq!(fs::read(&path).unwrap(), original);
    for (name, bytes) in [
        (
            "bom.txt",
            [b"\xef\xbb\xbf".as_slice(), "中文\nsecond\n".as_bytes()].concat(),
        ),
        (
            "utf16.txt",
            "alpha\n中文\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
        (
            "gb18030.txt",
            encoding_rs::GB18030
                .encode("中文😀\nsecond\n")
                .0
                .into_owned(),
        ),
    ] {
        fs::write(ctx.workspace.join(name), bytes).unwrap();
        let first = tools::execute("read", &json!({"path":name,"limit":1,"cursor":0}), &ctx).await;
        assert!(first.success, "{name}: {:?}", first.error);
        assert!(!first.output.contains('\u{feff}'));
        let cursor = first
            .output
            .split("next_cursor=")
            .nth(1)
            .unwrap()
            .trim_end_matches(']')
            .parse::<u64>()
            .unwrap();
        let next = tools::execute(
            "read",
            &json!({"path":name,"offset":2,"cursor":cursor}),
            &ctx,
        )
        .await;
        assert!(next.success, "{name}: {:?}", next.error);
        assert!(next.output.starts_with("2\t"));
    }
}

fn t1_pdf_fixture() -> Vec<u8> {
    let stream = "BT /F1 12 Tf 72 720 Td (PDF text layer) Tj ET";
    let objects=["<< /Type /Catalog /Pages 2 0 R >>".into(),"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".into(),"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),format!("<< /Length {} >>\nstream\n{stream}\nendstream",stream.len())];
    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = vec![0];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
    }
    let xref = pdf.len();
    pdf.push_str("xref\n0 6\n0000000000 65535 f \n");
    for offset in &offsets[1..] {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    ));
    pdf.into_bytes()
}

#[tokio::test]
async fn t1_reads_pdf_text_and_keeps_existing_image_blocks() {
    let (_dir, ctx) = fixture();
    fs::write(ctx.workspace.join("text.pdf"), t1_pdf_fixture()).unwrap();
    let pdf = tools::execute("read", &json!({"path":"text.pdf"}), &ctx).await;
    assert!(pdf.success, "{:?}", pdf.error);
    assert!(pdf.output.contains("PDF text layer"));
    assert!(!pdf.output.contains("%PDF-1.4"));
    image::RgbaImage::new(2, 2)
        .save(ctx.workspace.join("one.png"))
        .unwrap();
    let image = tools::execute("read", &json!({"path":"one.png"}), &ctx).await;
    assert!(image.success, "{:?}", image.error);
    assert_eq!(image.images.len(), 1);
    assert!(matches!(&image.images[0],Block::Media{kind,..} if kind=="image"));
}

#[test]
fn u3_terminal_text_removes_sequences_without_fragments() {
    for (raw, expected) in [
        ("\u{1b}[31m中文\u{1b}[0m\u{1b}[K\n", "中文\n"),
        (
            "\u{1b}]8;;https://example.com\u{1b}\\label\u{1b}]8;;\u{7}",
            "label",
        ),
        ("start\u{1b}[?25l\u{1b}[2Jend\u{1b}[31", "startend"),
        ("10%\r100%\r\nnext\tline", "100%\nnext\tline"),
        ("\u{9b}31mred\u{9b}0m\u{1b}Pignored\u{1b}\\", "red"),
    ] {
        assert_eq!(util::terminal_text(raw), expected);
    }
}

#[test]
fn r27_rejects_invalid_tool_timeout_configuration() {
    let (_dir, ctx) = fixture();
    let configuration = Configuration {
        path: ctx.workspace.join("timeouts.json"),
    };
    for timeouts in [
        json!([]),
        json!({"read":0}),
        json!({"read":-1}),
        json!({"read":3601}),
        json!({"read":"30"}),
    ] {
        fs::write(
            &configuration.path,
            serde_json::to_vec(&json!({"services":[],"agents":[],"toolTimeouts":timeouts}))
                .unwrap(),
        )
        .unwrap();
        assert!(configuration.read().is_err(), "{timeouts}");
    }
    configuration
        .write(&json!({"services":[],"agents":[],"toolTimeouts":{"grep":600}}))
        .unwrap();
    assert_eq!(
        configuration.tool_timeout("grep").unwrap(),
        Some(std::time::Duration::from_secs(600))
    );
    assert_eq!(configuration.tool_timeout("read").unwrap(), None);
}

#[test]
fn sh7_retention_preserves_old_session_history_and_unknown_files() {
    let (_dir, ctx) = fixture();
    let history = ctx
        .store
        .spill(&ctx.session, "history", "archived conversation")
        .unwrap();
    let output = ctx
        .store
        .spill(&ctx.session, "tool_output", "old output")
        .unwrap();
    let history_path = ctx.store.artifact_path(&ctx.session, &history).unwrap();
    let output_path = ctx.store.artifact_path(&ctx.session, &output).unwrap();
    let unknown = output_path.parent().unwrap().join("unknown.txt");
    fs::write(&unknown, "unregistered").unwrap();
    ctx.store
        .with(|c| {
            c.execute(
                "UPDATE artifacts SET created_at='2000-01-01T00:00:00+00:00'",
                [],
            )?;
            c.execute("DROP TABLE artifact_access", [])?;
            Ok(())
        })
        .unwrap();
    let reopened = Store::open(&ctx.store.path).unwrap();
    assert!(history_path.exists());
    assert!(!output_path.exists());
    assert!(unknown.exists());
    assert!(reopened.require(&ctx.session).is_ok());
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
fn usage_is_available_if_any_response_reports_it() {
    let missing = Usage {
        available: false,
        ..Usage::default()
    };
    let measured = Usage {
        input_tokens: 10,
        available: true,
        ..Usage::default()
    };
    let mut usage = Usage::default();
    usage.add(&missing);
    assert!(!usage.available);
    usage.add(&measured);
    assert!(usage.available);
    usage.add(&missing);
    assert!(usage.available);
    assert_eq!(usage.input_tokens, 10);
    let legacy: Usage = serde_json::from_value(json!({"input_tokens": 1})).unwrap();
    assert!(
        legacy.available,
        "old serialized usage without the field keeps its original default"
    );
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
    assert_eq!(tools::policy("powershell", "accept_edits"), "ask");
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
    assert!(
        tools::execute("read", &json!({"path":"a.txt"}), &ctx)
            .await
            .success
    );
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
    fs::write(ctx.workspace.join("binary.txt"), b"\xff\xfe\x00").unwrap();
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
                input_error: None,
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
                    input_error: None,
                    id: "a".into(),
                    name: "read".into(),
                    input: json!({}),
                },
                Block::ToolUse {
                    input_error: None,
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
            _ => value
                .to_string()
                .chars()
                .map(|ch| if ch.is_ascii() { 1 } else { 3 })
                .sum::<u64>(),
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
            input_error: None,
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
                    input_error: None,
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
    let mut anthropic = provider
        .payload("system", &messages, &[], &ctx.attachments)
        .unwrap();
    if anthropic["system"].is_array() {
        anthropic["system"] = anthropic["system"][0]["text"].clone();
    }
    for message in anthropic["messages"].as_array_mut().unwrap() {
        for block in message["content"].as_array_mut().unwrap() {
            block.as_object_mut().unwrap().remove("cache_control");
        }
    }
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
        assert!(
            tools::execute("read", &json!({"path":name}), &ctx)
                .await
                .success
        );
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

#[tokio::test]
async fn x1_compaction_uses_structured_summary_without_consuming_task_turns() {
    let summary = "## Goal\n完成修复\n## Constraints & Preferences\n保留接口\n## Progress\n### Done\n已改 agent/context.rs，cargo test 通过\n### In Progress\n文档\n### Blocked\n无\n## Key Decisions\n选择兼容方案\n## Next Steps\n提交\n## Critical Context\n### Read Files\nagent/tests.rs\n### Modified Files\nagent/context.rs";
    let mut h = Harness::new(json!({"turns":[{"text":"task reply"}]})).await;
    h.runtime.provider.fake.as_mut().unwrap()["summary"] = json!(summary);
    h.runtime.provider.index = 3;
    h.runtime.messages = vec![
        Message::user("original constraints"),
        Message::user("older task"),
        Message {
            role: "assistant".into(),
            content: vec![Block::text(
                "已改 agent/context.rs，选择兼容方案，cargo test 通过；下一步提交。".repeat(300),
            )],
        },
        Message::user("latest task"),
    ];
    h.runtime
        .store
        .replace_messages(&h.runtime.session, &h.runtime.messages)
        .unwrap();
    assert!(h.runtime.compact(&h.hooks).await.unwrap());
    assert!(
        h.runtime
            .messages
            .iter()
            .any(|m| m.text().contains(summary))
    );
    assert_eq!(h.runtime.provider.index, 3);
    let events = h.runtime.store.events(&h.runtime.session, -1).unwrap();
    assert!(
        events.iter().any(|e| e.r#type == "context_compacted"
            && e.data["summarized_units"].as_u64().unwrap_or(0) > 0)
    );
    // Repeated compaction updates the previous summary rather than stacking it.
    h.runtime
        .messages
        .push(Message::user("new history ".repeat(1000)));
    h.runtime.messages.push(Message::user("next task"));
    assert!(h.runtime.compact(&h.hooks).await.unwrap());
    assert_eq!(
        h.runtime
            .messages
            .iter()
            .filter(|m| m.text().contains("## Goal"))
            .count(),
        1
    );
    h.runtime.provider.fake.as_mut().unwrap()["summary_error"] = json!("offline");
    h.runtime
        .messages
        .push(Message::user("another history ".repeat(2000)));
    h.runtime.messages.push(Message::user("latest"));
    assert!(h.runtime.compact(&h.hooks).await.unwrap());
    assert!(
        h.runtime
            .messages
            .iter()
            .any(|m| m.text().contains(summary))
    );
}

#[tokio::test]
async fn x1_summary_failure_falls_back_to_archived_history() {
    let mut h = Harness::new(json!({"turns":[]})).await;
    h.runtime.provider.fake.as_mut().unwrap()["summary_error"] = json!("unavailable");
    h.runtime.messages = vec![
        Message::user("original"),
        Message::user("old ".repeat(2000)),
        Message::user("recent"),
    ];
    assert!(h.runtime.compact(&h.hooks).await.unwrap());
    assert!(
        h.runtime
            .messages
            .iter()
            .any(|m| m.text().contains("[artifact:"))
    );
    assert_eq!(h.runtime.messages[0].text(), "original");
    assert_eq!(h.runtime.provider.index, 0);
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
    let command = "echo 'hello'";
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
async fn sh3_shell_defaults_disable_color() {
    let (_dir, ctx) = fixture();
    let result = tools::run_command("node -e \"console.log([process.env.NO_COLOR,process.env.TERM,process.env.CARGO_TERM_COLOR].join(','))\"", &ctx.workspace, 10.0, &ctx).await.unwrap();
    let expected = [
        ("NO_COLOR", "1"),
        ("TERM", "dumb"),
        ("CARGO_TERM_COLOR", "never"),
    ]
    .map(|(key, default)| {
        std::env::var(key)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default.into())
    })
    .join(",");
    assert_eq!(result.output, expected);
}

#[tokio::test]
async fn sh8_command_preview_keeps_first_error_and_last_line() {
    let (_dir, ctx) = fixture();
    for padding in 0..11 {
        fs::write(ctx.workspace.join("output.cjs"), format!("process.stdout.write('first compiler error\\n'+'x'.repeat({padding})+'中文line\\n'.repeat(50000)+'last line\\n')")).unwrap();
        let outcome = tools::run_command("node output.cjs", &ctx.workspace, 10.0, &ctx)
            .await
            .unwrap();
        assert!(outcome.success, "{:?}", outcome.error);
        assert!(outcome.output.starts_with("first compiler error\n"));
        assert!(outcome.output.ends_with("last line"));
        assert!(outcome.output.contains("[artifact:"));
        let (head, tail) = outcome.output.split_once("...[middle output").unwrap();
        assert!(head.contains("中文line"), "head padding={padding}");
        assert!(tail.contains("中文line"), "tail padding={padding}");
        assert!(outcome.output.len() <= 50 * 1024);
        assert!(outcome.output.lines().count() <= 2000);
        let id = outcome
            .output
            .split("[artifact:")
            .nth(1)
            .unwrap()
            .split(']')
            .next()
            .unwrap();
        let full = fs::read_to_string(ctx.store.artifact_path(&ctx.session, id).unwrap()).unwrap();
        assert_eq!(full.lines().count(), 50002);
    }
}

#[tokio::test]
async fn sh5_redirected_descendant_survives_shell_exit() {
    let (_dir, ctx) = fixture();
    fs::write(
        ctx.workspace.join("server.cjs"),
        "require('fs').writeFileSync('started','yes');setTimeout(()=>require('fs').writeFileSync('survived','yes'),2800)",
    )
    .unwrap();
    fs::write(
        ctx.workspace.join("launch.cjs"),
        "const c=require('child_process').spawn(process.execPath,['server.cjs'],{stdio:'ignore',detached:process.platform==='win32'});console.log(c.pid);c.on('error',console.error);c.unref()",
    )
    .unwrap();
    let result = tools::run_command("node launch.cjs", &ctx.workspace, 10.0, &ctx)
        .await
        .unwrap();
    assert!(result.success, "{:?}", result.error);
    for _ in 0..200 {
        if ctx.workspace.join("survived").exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!(
        "redirected descendant was killed: {result:?}; started={}",
        ctx.workspace.join("started").exists()
    );
}

#[tokio::test]
async fn sh5_descendant_holding_output_is_stopped() {
    let (_dir, ctx) = fixture();
    fs::write(ctx.workspace.join("launch.cjs"), "require('child_process').spawn(process.execPath,['-e',\"console.log('child');setTimeout(()=>require('fs').writeFileSync('leaked','yes'),4000)\"],{stdio:['ignore','inherit','inherit'],detached:process.platform==='win32'}).unref()").unwrap();
    let result = tools::run_command("node launch.cjs", &ctx.workspace, 10.0, &ctx)
        .await
        .unwrap();
    assert!(!result.success, "{result:?}");
    assert!(result.error.unwrap().contains("kept output open"));
    assert!(result.output.contains("child"));
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    assert!(!ctx.workspace.join("leaked").exists());
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
    let result = task
        .await
        .unwrap()
        .expect("cancelled commands return partial output");
    assert!(!result.success);
    assert!(result.error.unwrap().contains("cancelled"));
    fs::remove_dir_all(&cwd).unwrap();
}

#[tokio::test]
async fn sh6_cancel_preserves_partial_command_output() {
    let (_dir, ctx) = fixture();
    let child_ctx = ctx.clone();
    let command = "node -e \"console.log('first error');require('fs').writeFileSync('ready','1');setInterval(()=>{},1000)\"";
    let task = tokio::spawn(async move {
        tools::run_command(command, &child_ctx.workspace, 10.0, &child_ctx).await
    });
    for _ in 0..500 {
        if ctx.workspace.join("ready").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(ctx.workspace.join("ready").exists());
    ctx.cancel.cancel();
    let outcome = task.await.unwrap().unwrap();
    assert!(!outcome.success);
    assert!(outcome.output.contains("first error"));
    assert!(outcome.error.unwrap().contains("cancelled"));
}

#[cfg(unix)]
#[tokio::test]
async fn sh6_signal_has_a_name_instead_of_exit_minus_one() {
    let (_dir, ctx) = fixture();
    let outcome = tools::run_command(
        "printf 'before signal\\n'; kill -TERM $$",
        &ctx.workspace,
        10.0,
        &ctx,
    )
    .await
    .unwrap();
    assert!(!outcome.success);
    assert!(outcome.output.contains("before signal"));
    assert_eq!(outcome.signal.as_deref(), Some("SIGTERM"));
    assert!(outcome.error.unwrap().contains("SIGTERM"));
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
    /// Reload the session from the store, as `/resume` does.
    async fn reopen(&mut self) -> anyhow::Result<()> {
        let configuration = self.runtime.catalog.lock().await.configuration.clone();
        let settings = self.hooks.settings.lock().unwrap().clone();
        let mut runtime = crate::runtime::Runtime::new(
            &self.runtime.workspace,
            "fake",
            Some(&self.runtime.session),
            self.runtime.store.clone(),
            configuration,
            &settings,
        )
        .await?;
        if let Some(script) = self.runtime.provider.fake.clone() {
            runtime.set_script(script)?;
        }
        self.runtime = runtime;
        Ok(())
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

#[tokio::test]
async fn provider_retry_does_not_use_error_message_keywords() {
    let mut h = Harness::new(json!({"turns":[
        {"error":"connection policy rejected the request"},
        {"text":"must not retry"}
    ]}))
    .await;
    let result = h.run(Some("inspect")).await.unwrap();
    assert_eq!(result.exit_reason, "provider_error");
    assert!(result.error.unwrap().contains("connection policy"));
    assert!(!h.event_types().contains(&"provider_retry".to_string()));
}

#[tokio::test]
async fn provider_error_with_deadline_text_is_not_a_turn_deadline() {
    let mut h = Harness::new(json!({"turns":[{"error":"turn deadline exceeded"}]})).await;
    let result = h.run(Some("inspect")).await.unwrap();
    assert_eq!(result.exit_reason, "provider_error");
}

#[tokio::test]
async fn dynamic_instruction_reminder_survives_manual_compaction() {
    let mut h = Harness::new(json!({"turns":[
        {"tool_calls":[{"name":"skill_load","arguments":{"name":"compact-helper"}}]},
        {"text":"ready"}
    ]}))
    .await;
    let skill = h.runtime.workspace.join(".minicode/skills/compact-helper");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("SKILL.md"), "---\nname: compact-helper\ndescription: compact helper\n---\nfull-skill-context-must-survive").unwrap();
    h.reopen().await.unwrap();
    assert_eq!(
        h.run(Some("original task")).await.unwrap().exit_reason,
        "completed"
    );
    let session = h.runtime.session.clone();
    let before = h.runtime.store.frozen_prompt(&session).unwrap().unwrap();
    for i in 0..8 {
        let message = Message::user(format!("history {i}: {}", "filler ".repeat(400)));
        h.runtime.store.append(&session, &message).unwrap();
        h.runtime.messages.push(message);
    }
    h.runtime.message_revision = Some(h.runtime.store.message_revision(&session).unwrap());
    assert!(h.runtime.compact(&h.hooks).await.unwrap());
    assert!(
        h.runtime
            .messages
            .last()
            .unwrap()
            .text()
            .contains("full-skill-context-must-survive")
    );
    let after = h.runtime.store.frozen_prompt(&session).unwrap().unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn frozen_system_is_not_exposed_as_a_session_event() {
    let mut h = Harness::new(json!({"turns":[{"text":"done"}]})).await;
    fs::write(
        h.runtime.workspace.join("AGENTS.md"),
        "private-root-instruction-fixture",
    )
    .unwrap();
    h.reopen().await.unwrap();
    assert_eq!(h.run(Some("start")).await.unwrap().exit_reason, "completed");
    let events = h.runtime.store.events(&h.runtime.session, -1).unwrap();
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private-root-instruction-fixture")
    );
}

#[test]
fn frozen_prompt_is_immutable_and_deleted_with_its_session() {
    let (_dir, ctx) = fixture();
    let sources = json!([{"path":"AGENTS.md","sha256":"first"}]);
    let first = ctx
        .store
        .freeze_prompt(&ctx.session, "original", &sources)
        .unwrap();
    let second = ctx
        .store
        .freeze_prompt(&ctx.session, "replacement", &json!([]))
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(second.0, "original");
    ctx.store.delete(&ctx.session).unwrap();
    assert!(ctx.store.frozen_prompt(&ctx.session).unwrap().is_none());
}

#[tokio::test]
async fn legacy_active_skill_is_restored_as_a_reminder() {
    let mut h = Harness::new(json!({"turns":[{"text":"done"}]})).await;
    let skill = h.runtime.workspace.join(".minicode/skills/legacy-helper");
    fs::create_dir_all(&skill).unwrap();
    let path = skill.join("SKILL.md");
    fs::write(
        &path,
        "---\nname: legacy-helper\ndescription: helper\n---\nlegacy-skill-content",
    )
    .unwrap();
    let source = crate::extensions::Source::read(&path, 64_000).unwrap();
    let session = h.runtime.session.clone();
    h.runtime
        .store
        .event(
            &session,
            "session_start",
            &json!({"project_instructions":[],"active_skills":{}}),
        )
        .unwrap();
    h.runtime
        .store
        .event(
            &session,
            "skill_activated",
            &json!({"name":"legacy-helper","path":path,"sha256":source.digest}),
        )
        .unwrap();
    h.reopen().await.unwrap();
    assert_eq!(
        h.run(Some("continue")).await.unwrap().exit_reason,
        "completed"
    );
    assert!(h.runtime.messages.iter().any(
        |m| m.text().contains("legacy-skill-content") && m.text().contains("<system-reminder>")
    ));
    let frozen = h.runtime.store.frozen_prompt(&session).unwrap().unwrap();
    assert!(!frozen.0.contains("legacy-skill-content"));
}

#[tokio::test]
async fn discovered_instructions_recover_after_interruption_before_reminder() {
    let mut h = Harness::new(json!({"turns":[{"text":"first"},{"text":"continued"}]})).await;
    assert_eq!(h.run(Some("start")).await.unwrap().exit_reason, "completed");
    let nested = h.runtime.workspace.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("AGENTS.md"), "recovered-nested-rules").unwrap();
    fs::write(nested.join("file.txt"), "data").unwrap();
    let sources = h
        .runtime
        .catalog
        .lock()
        .await
        .discover_instructions("nested/file.txt")
        .unwrap();
    for source in sources {
        h.runtime
            .store
            .event(&h.runtime.session, "project_instructions", &source)
            .unwrap();
    }
    h.reopen().await.unwrap();
    assert_eq!(
        h.run(Some("continue")).await.unwrap().exit_reason,
        "completed"
    );
    assert!(
        h.runtime
            .messages
            .iter()
            .any(|m| m.text().contains("recovered-nested-rules")
                && m.text().contains("<system-reminder>"))
    );
}

#[tokio::test]
async fn delegate_tool_description_has_stable_agent_order() {
    let (_dir, ctx) = fixture();
    let catalog = ctx.catalog.lock().await;
    let agents = vec![
        json!({"name":"zeta","enabled":true}),
        json!({"name":"alpha","enabled":true}),
    ];
    let mut reversed = agents.clone();
    reversed.reverse();
    let first = tools::specs(&catalog, &ctx.mcp, &agents, true);
    let second = tools::specs(&catalog, &ctx.mcp, &reversed, true);
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(second).unwrap()
    );
}

#[tokio::test]
async fn resumed_legacy_session_recovers_available_usage_from_events() {
    let mut h =
        Harness::new(json!({"turns":[{"text":"done","input_tokens":0,"output_tokens":0}]})).await;
    let first = h.run(Some("inspect")).await.unwrap();
    assert!(first.total_usage.available);
    h.runtime
        .store
        .update(&h.runtime.session, &json!({"usage_available":false}))
        .unwrap();
    h.reopen().await.unwrap();
    assert!(
        h.runtime.usage.available,
        "available zero-token usage must survive resume"
    );
    assert_eq!(
        h.runtime.store.require(&h.runtime.session).unwrap()["usage_available"],
        true
    );
}

#[tokio::test]
async fn empty_legacy_session_does_not_claim_usage_is_available() {
    let mut h = Harness::new(json!({"turns":[]})).await;
    h.runtime
        .store
        .update(&h.runtime.session, &json!({"usage_available":true}))
        .unwrap();
    h.reopen().await.unwrap();
    assert!(!h.runtime.usage.available);
    assert_eq!(
        h.runtime.store.require(&h.runtime.session).unwrap()["usage_available"],
        false
    );
}

#[tokio::test]
async fn recovered_child_usage_is_available_even_if_legacy_summary_says_unavailable() {
    let mut h = Harness::new(json!({"turns":[]})).await;
    assert_eq!(
        h.runtime.store.require(&h.runtime.session).unwrap()["usage_available"],
        false
    );
    let child = h
        .runtime
        .store
        .create(&h.runtime.workspace.to_string_lossy(), "fake", "fake")
        .unwrap();
    h.runtime
        .store
        .event(
            &h.runtime.session,
            "subagent_start",
            &json!({"child_session_id":child}),
        )
        .unwrap();
    h.runtime
        .store
        .event(
            &child,
            "assistant_message",
            &json!({"usage":{"available":true,"input_tokens":0,"output_tokens":0}}),
        )
        .unwrap();
    h.runtime
        .store
        .update(&child, &json!({"rounds":1,"usage_available":false}))
        .unwrap();
    h.reopen().await.unwrap();
    assert!(h.runtime.usage.available);
    assert_eq!(
        h.runtime.store.require(&h.runtime.session).unwrap()["usage_available"],
        true
    );
}

#[test]
fn provider_error_retry_classification_uses_variants_and_status() {
    use crate::provider::ProviderError;
    for status in [408, 429, 500, 503, 529] {
        assert!(
            ProviderError::Http {
                status,
                body: String::new(),
                retry_after: None
            }
            .is_retryable(),
            "{status}"
        );
    }
    for status in [400, 401, 403, 404] {
        assert!(
            !ProviderError::Http {
                status,
                body: String::new(),
                retry_after: None
            }
            .is_retryable(),
            "{status}"
        );
    }
    assert!(
        ProviderError::Timeout {
            idle: std::time::Duration::from_secs(1)
        }
        .is_retryable()
    );
    assert!(!ProviderError::Protocol("connection HTTP 503".into()).is_retryable());
}

// R40: every runtime path that appends a user notice right after a user
// tool_result must still produce a strictly alternating Anthropic payload.
#[tokio::test]
async fn anthropic_roles_alternate_after_truncated_response() {
    let mut h = Harness::new(json!({"turns":[
        {"tool_calls":[{"name":"read","arguments":{"path":"README.md"}}]},
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
    assert_eq!(
        roles,
        [
            "user",
            "assistant",
            "user",
            "assistant",
            "user",
            "assistant"
        ]
    );
    let messages = h.anthropic_messages();
    assert_strict_anthropic_roles(&messages);
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert!(
        messages[4]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("write")
    );
    assert!(!h.root.join("x.txt").exists());
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
            .any(|m| m.role == "user" && m.text().contains("后台任务已完成"))
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
                input_error: None,
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
    h.runtime.message_revision = Some(h.runtime.store.message_revision(&session).unwrap());
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
                    input_error: None,
                    id: "a".into(),
                    name: "read".into(),
                    input: json!({}),
                },
                Block::ToolUse {
                    input_error: None,
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
    let mut normalized = payload["messages"].clone();
    for message in normalized.as_array_mut().unwrap() {
        for block in message["content"].as_array_mut().unwrap() {
            block.as_object_mut().unwrap().remove("cache_control");
        }
    }
    assert_eq!(
        normalized,
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

// R1: instruction or skill drift on resume re-pins the current version and
// tells the model, instead of making the session impossible to resume.
#[tokio::test]
async fn resume_repins_changed_instructions_and_skills() {
    let mut h = Harness::new(json!({"turns":[
        {"tool_calls":[{"name":"skill_load","arguments":{"name":"helper"}}]},
        {"text":"first"},
        {"text":"second"},
        {"text":"third"}
    ]}))
    .await;
    let workspace = h.runtime.workspace.clone();
    let skill = workspace.join(".minicode/skills/helper");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: helper\ndescription: Helper\n---\nhelper body",
    )
    .unwrap();
    fs::write(workspace.join("AGENTS.md"), "original rules").unwrap();
    h.reopen().await.unwrap();
    assert_eq!(h.run(Some("go")).await.unwrap().exit_reason, "completed");

    fs::write(workspace.join("AGENTS.md"), "changed rules").unwrap();
    fs::remove_dir_all(&skill).unwrap();
    h.reopen().await.unwrap();
    assert_eq!(h.run(Some("again")).await.unwrap().exit_reason, "completed");
    let events = h.runtime.store.events(&h.runtime.session, -1).unwrap();
    let changed = events
        .iter()
        .filter(|e| e.r#type == "project_instructions_changed")
        .collect::<Vec<_>>();
    assert_eq!(changed.len(), 1);
    let statuses = changed[0].data["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["status"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(statuses, ["changed", "removed"]);
    let reminder = h
        .runtime
        .messages
        .iter()
        .map(Message::text)
        .find(|t| t.contains("<system-reminder>"))
        .expect("the model is told about the change");
    assert!(reminder.contains("AGENTS.md") && reminder.contains("helper"));
    assert!(
        h.runtime
            .messages
            .iter()
            .any(|m| m.text().contains("<system-reminder>") && m.text().contains("changed rules"))
    );
    {
        let catalog = h.runtime.catalog.lock().await;
        assert!(catalog.active.is_empty());
        assert!(catalog.prompt(&[]).unwrap().contains("changed rules"));
    }

    // The new version is pinned: an unchanged resume reports nothing new.
    h.reopen().await.unwrap();
    fs::remove_file(workspace.join("AGENTS.md")).unwrap();
    h.reopen().await.unwrap();
    assert_eq!(h.run(Some("third")).await.unwrap().exit_reason, "completed");
    let events = h.runtime.store.events(&h.runtime.session, -1).unwrap();
    let changed = events
        .iter()
        .filter(|e| e.r#type == "project_instructions_changed")
        .collect::<Vec<_>>();
    assert_eq!(changed.len(), 2);
    assert_eq!(changed[1].data["changes"][0]["status"], "deleted");
    assert!(
        !h.runtime
            .catalog
            .lock()
            .await
            .prompt(&[])
            .unwrap()
            .contains("changed rules")
    );
}

// R1: a paused subagent resumed after the instructions changed must not turn
// the parent run into an internal_error.
#[tokio::test]
async fn resumed_subagent_repins_changed_instructions() {
    let delegate =
        json!({"tool_calls":[{"name":"delegate","arguments":{"kind":"explore","task":"inspect"}}]});
    let mut h = Harness::new(json!({"turns":[
        delegate,
        {"error":"provider HTTP 400 bad request"},
        {"text":"parent paused"},
        delegate,
        {"text":"{\"summary\":\"ok\",\"findings\":[],\"evidence_refs\":[],\"unresolved\":[]}"},
        {"text":"parent done"}
    ]}))
    .await;
    let workspace = h.runtime.workspace.clone();
    fs::write(workspace.join("AGENTS.md"), "original rules").unwrap();
    h.reopen().await.unwrap();
    assert_eq!(h.run(Some("first")).await.unwrap().exit_reason, "completed");
    let child = h
        .runtime
        .store
        .events(&h.runtime.session, -1)
        .unwrap()
        .into_iter()
        .find(|e| e.r#type == "subagent_result")
        .unwrap();
    assert_ne!(child.data["exit_reason"], "completed");
    let child = child.data["child_session_id"].as_str().unwrap().to_string();

    fs::write(workspace.join("AGENTS.md"), "changed rules").unwrap();
    h.reopen().await.unwrap();
    assert_eq!(h.run(Some("again")).await.unwrap().exit_reason, "completed");
    let child_events = h.runtime.store.events(&child, -1).unwrap();
    assert!(
        child_events
            .iter()
            .any(|e| e.r#type == "project_instructions_changed")
    );
    assert!(
        h.runtime
            .store
            .messages(&child)
            .unwrap()
            .iter()
            .any(|m| m.text().contains("<system-reminder>"))
    );
}

// T3: edit / write must not clobber a file the session has not read, or one
// that changed on disk after the session last saw it.
#[tokio::test]
async fn edit_and_write_require_a_current_read() {
    let (_dir, ctx) = fixture();
    let large = "x".repeat(300_000) + "needle";
    fs::write(ctx.workspace.join("user.txt"), "original\n").unwrap();
    fs::write(ctx.workspace.join("large.txt"), &large).unwrap();
    for (name, args) in [
        ("write", json!({"path":"user.txt","content":"clobbered"})),
        (
            "edit",
            json!({"path":"user.txt","old_text":"original","new_text":"edited"}),
        ),
        (
            "edit",
            json!({"path":"large.txt","old_text":"needle","new_text":"pin"}),
        ),
    ] {
        let out = tools::execute(name, &args, &ctx).await;
        assert!(!out.success, "{name} of an unread file must fail");
        let error = out.error.unwrap_or_default();
        assert!(error.contains("read"), "{error}");
    }
    assert_eq!(
        fs::read_to_string(ctx.workspace.join("user.txt")).unwrap(),
        "original\n"
    );
    let created = tools::execute("write", &json!({"path":"new.txt","content":"new"}), &ctx).await;
    assert!(created.success, "{:?}", created.error);
    // A file written by the session can be edited again without a read.
    let again = tools::execute(
        "edit",
        &json!({"path":"new.txt","old_text":"new","new_text":"newer"}),
        &ctx,
    )
    .await;
    assert!(again.success, "{:?}", again.error);

    for path in ["user.txt", "large.txt"] {
        assert!(
            tools::execute("read", &json!({"path":path}), &ctx)
                .await
                .success
        );
    }
    fs::write(ctx.workspace.join("user.txt"), "changed in the IDE\n").unwrap();
    fs::write(ctx.workspace.join("large.txt"), large.clone() + "!").unwrap();
    for (name, args) in [
        ("write", json!({"path":"user.txt","content":"clobbered"})),
        (
            "edit",
            json!({"path":"user.txt","old_text":"changed","new_text":"edited"}),
        ),
        (
            "edit",
            json!({"path":"large.txt","old_text":"needle","new_text":"pin"}),
        ),
    ] {
        let out = tools::execute(name, &args, &ctx).await;
        assert!(!out.success, "{name} after an external change must fail");
        let error = out.error.unwrap_or_default();
        assert!(error.contains("外部修改"), "{error}");
    }
    assert_eq!(
        fs::read_to_string(ctx.workspace.join("user.txt")).unwrap(),
        "changed in the IDE\n"
    );
    assert!(
        tools::execute("read", &json!({"path":"user.txt"}), &ctx)
            .await
            .success
    );
    let edited = tools::execute(
        "edit",
        &json!({"path":"user.txt","old_text":"changed","new_text":"edited"}),
        &ctx,
    )
    .await;
    assert!(edited.success, "{:?}", edited.error);
}

#[tokio::test]
async fn concurrent_edits_of_one_file_are_serialized() {
    let (_dir, ctx) = fixture();
    for round in 0..20 {
        let words = (0..8).map(|i| format!("w{i}")).collect::<Vec<_>>();
        fs::write(ctx.workspace.join("shared.txt"), words.join(" ")).unwrap();
        assert!(
            tools::execute("read", &json!({"path":"shared.txt"}), &ctx)
                .await
                .success
        );
        let edits = words.iter().map(|word| {
            let ctx = ctx.clone();
            let args = json!({"path":"shared.txt","old_text":word,"new_text":word.to_uppercase()});
            async move { tools::execute("edit", &args, &ctx).await }
        });
        for out in futures_util::future::join_all(edits).await {
            assert!(out.success, "round {round}: {:?}", out.error);
        }
        assert_eq!(
            fs::read_to_string(ctx.workspace.join("shared.txt")).unwrap(),
            words.join(" ").to_uppercase()
        );
    }
}

// T7: one unreadable file must not abort the search, and grep must never map
// workspace files (a concurrent truncation of a mapping is a SIGBUS crash).
#[tokio::test]
async fn grep_skips_unreadable_files_and_reports_them() {
    let (_dir, ctx) = fixture();
    fs::write(ctx.workspace.join("a.txt"), "needle one\n").unwrap();
    fs::write(ctx.workspace.join("b.txt"), "needle locked\n").unwrap();
    fs::write(ctx.workspace.join("c.txt"), "needle three\n").unwrap();
    // A mandatory lock on Windows: open succeeds, every read fails.
    #[cfg(windows)]
    let _held = {
        let file = fs::File::options()
            .read(true)
            .write(true)
            .open(ctx.workspace.join("b.txt"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&file).unwrap();
        file
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            ctx.workspace.join("b.txt"),
            fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        if fs::read(ctx.workspace.join("b.txt")).is_ok() {
            return; // Running as root: nothing is unreadable.
        }
    }
    let out = tools::execute("grep", &json!({"pattern":"needle"}), &ctx).await;
    assert!(out.success, "{:?}", out.error);
    assert!(out.output.contains("a.txt:1: needle one"), "{}", out.output);
    assert!(
        out.output.contains("c.txt:1: needle three"),
        "{}",
        out.output
    );
    assert!(!out.output.contains("locked"), "{}", out.output);
    assert!(out.output.contains("跳过 1 个"), "{}", out.output);
}

#[cfg(unix)]
#[tokio::test]
async fn grep_survives_concurrent_truncation() {
    let (_dir, ctx) = fixture();
    let path = ctx.workspace.join("growing.log");
    let content = "line needle\n".repeat(200_000);
    fs::write(&path, &content).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (path, stop) = (path.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.set_len(0).unwrap();
                file.set_len(content.len() as u64).unwrap();
            }
        })
    };
    for _ in 0..50 {
        let out = tools::execute(
            "grep",
            &json!({"pattern":"needle","max_results":1000}),
            &ctx,
        )
        .await;
        assert!(out.success, "{:?}", out.error);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().unwrap();
}

// SEC1: MCP servers declared by a workspace plugin never start until the user
// trusts that plugin's current fingerprint; the trust lives outside the
// workspace, and changing the plugin asks again.
fn untrusted_plugin(h: &Harness, description: &str) -> std::path::PathBuf {
    let marker = h.root.join("started");
    let dir = h.runtime.workspace.join(".minicode/plugins/evil");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("plugin.json"),
        serde_json::to_vec(&json!({
            "name":"evil","version":"1.0.0","minicode_version":crate::VERSION,"enabled":true,
            "description":description,
            "mcp_servers":[{"name":"evil","command":"node","args":["-e",format!("require('fs').writeFileSync({:?},'x')",marker.to_string_lossy())],"timeout_s":5}]
        }))
        .unwrap(),
    )
    .unwrap();
    marker
}
/// Answers every approval with `grant` and returns the requests it saw.
fn approver(h: &Harness, grant: bool) -> Arc<std::sync::Mutex<Vec<String>>> {
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let (approvals, recorded) = (h.hooks.approvals.clone(), seen.clone());
    tokio::spawn(async move {
        loop {
            let pending = approvals.lock().unwrap().drain().collect::<Vec<_>>();
            for (_, (tool, tx)) in pending {
                recorded.lock().unwrap().push(tool);
                let _ = tx.send(grant);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    seen
}

#[tokio::test]
async fn untrusted_workspace_plugins_start_no_processes() {
    let mut h = Harness::new(
        json!({"turns":[{"text":"one"},{"text":"two"},{"text":"three"},{"text":"four"}]}),
    )
    .await;
    let marker = untrusted_plugin(&h, "first");
    // Neither bypass mode nor "always allow" may grant workspace trust.
    h.hooks
        .settings
        .lock()
        .unwrap()
        .always_allow
        .insert("workspace_trust".into());
    h.reopen().await.unwrap();
    let asked = approver(&h, false);
    assert_eq!(h.run(Some("go")).await.unwrap().exit_reason, "completed");
    assert_eq!(*asked.lock().unwrap(), ["workspace_trust"]);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!marker.exists(), "an untrusted plugin started a process");
    assert!(!h.root.join("trusted-plugins.json").exists());
}

#[tokio::test]
async fn trusted_workspace_plugins_start_until_they_change() {
    let mut h =
        Harness::new(json!({"turns":[{"text":"one"},{"text":"two"},{"text":"three"}]})).await;
    let marker = untrusted_plugin(&h, "first");
    h.reopen().await.unwrap();
    let asked = approver(&h, true);
    assert_eq!(h.run(Some("go")).await.unwrap().exit_reason, "completed");
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(
        crate::tests::wait_for(|| marker.exists()).await,
        "the trusted plugin did not start"
    );
    assert!(h.root.join("trusted-plugins.json").exists());
    fs::remove_file(&marker).unwrap();

    // Trusted fingerprint: no question on the next run.
    h.reopen().await.unwrap();
    assert_eq!(h.run(Some("again")).await.unwrap().exit_reason, "completed");
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(crate::tests::wait_for(|| marker.exists()).await);
    fs::remove_file(&marker).unwrap();

    // A changed manifest is a different plugin: ask again.
    untrusted_plugin(&h, "second");
    h.reopen().await.unwrap();
    assert_eq!(
        h.run(Some("changed")).await.unwrap().exit_reason,
        "completed"
    );
    assert_eq!(asked.lock().unwrap().len(), 2);
}
async fn wait_for(check: impl Fn() -> bool) -> bool {
    for _ in 0..500 {
        if check() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    false
}

#[test]
fn message_revision_detects_same_length_changes_and_rolls_back_atomically() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path().join("sessions.db")).unwrap();
    let id = store.create("/repo", "fake", "fake").unwrap();
    store
        .replace_messages(&id, &[Message::user("first"), Message::user("last")])
        .unwrap();
    let (messages, version) = store.versioned_messages(&id).unwrap();
    assert_eq!(messages.len(), 2);
    // An old binary or another process writes the same database: triggers
    // must see updates even when count and last message are unchanged.
    let external = rusqlite::Connection::open(&store.path).unwrap();
    external
        .execute(
            "UPDATE messages SET content=? WHERE session_id=? AND seq=0",
            rusqlite::params![r#"[{"type":"text","text":"other"}]"#, id],
        )
        .unwrap();
    assert!(store.message_revision(&id).unwrap() > version);
    assert_eq!(store.messages(&id).unwrap()[0].text(), "other");
    let version = store.message_revision(&id).unwrap();
    let rollback_id = id.clone();
    assert!(
        store
            .transaction(move |db| -> anyhow::Result<()> {
                db.execute("DELETE FROM messages WHERE session_id=?", [&rollback_id])?;
                anyhow::bail!("rollback")
            })
            .is_err()
    );
    assert_eq!(store.message_revision(&id).unwrap(), version);
    assert_eq!(store.messages(&id).unwrap().len(), 2);
    store
        .replace_messages(&id, &[Message::user("changed"), Message::user("last")])
        .unwrap();
    assert!(store.message_revision(&id).unwrap() > version);
}

#[test]
fn reasoning_models_use_compatible_output_and_effort_parameters() {
    let (_dir, ctx) = fixture();
    let mut provider = crate::provider::Provider::new(
        "fake",
        "off",
        &Configuration {
            path: ctx.workspace.join("config.json"),
        },
    )
    .unwrap();
    for (model, effort, expected) in [
        ("gpt-5", "max", "high"),
        ("gpt-5.1", "xhigh", "high"),
        ("gpt-5.4", "max", "xhigh"),
        ("o3", "max", "high"),
        ("o4-mini", "xhigh", "high"),
    ] {
        provider.model = model.into();
        provider.effort = effort.into();
        let body = provider
            .payload("", &[Message::user("hi")], &[], &ctx.attachments)
            .unwrap();
        assert!(body.get("max_tokens").is_none(), "{model}: {body}");
        assert!(body["max_completion_tokens"].as_u64().is_some());
        assert_eq!(body["reasoning_effort"], expected);
    }
    provider.model = "gateway/custom".into();
    provider.effort = "max".into();
    let body = provider
        .payload("", &[Message::user("hi")], &[], &ctx.attachments)
        .unwrap();
    assert!(body["max_tokens"].as_u64().is_some());
    assert_eq!(body["reasoning_effort"], "max");

    let configuration = Configuration {
        path: ctx.workspace.join("compat.json"),
    };
    let saved = configuration
        .save_service(&json!({
            "name":"custom", "baseUrl":"https://example.test/v1", "apiStyle":"openai",
            "apiKey":"test-key", "models":[{
                "modelId":"custom", "contextWindow":128000, "maxOutputTokens":8192,
                "supportsEffort":true,
                "parameterCompat":{"maxCompletionTokens":true,"reasoningEffort":{"max":"high"}}
            }]
        }))
        .unwrap();
    let provider = crate::provider::Provider::new(
        saved["defaultModel"].as_str().unwrap(),
        "max",
        &configuration,
    )
    .unwrap();
    let body = provider
        .payload("", &[Message::user("hi")], &[], &ctx.attachments)
        .unwrap();
    assert!(body.get("max_tokens").is_none());
    assert!(body["max_completion_tokens"].as_u64().is_some());
    assert_eq!(body["reasoning_effort"], "high");
}

#[test]
fn anthropic_thinking_budget_respects_output_limit() {
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
    provider.max_output = 8192;
    provider.effort = "high".into();
    let body = provider
        .payload("", &[Message::user("hi")], &[], &ctx.attachments)
        .unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
    assert!(budget >= 1024 && budget < body["max_tokens"].as_u64().unwrap());
    provider.effort = "off".into();
    assert!(
        provider
            .payload("", &[Message::user("hi")], &[], &ctx.attachments)
            .unwrap()
            .get("thinking")
            .is_none()
    );
}

#[test]
fn anthropic_adaptive_models_use_supported_thinking_parameters() {
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
    provider.model = "claude-sonnet-4-6".into();
    provider.effort = "max".into();
    let body = provider
        .payload("", &[Message::user("hi")], &[], &ctx.attachments)
        .unwrap();
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert!(body["thinking"].get("budget_tokens").is_none());
    assert_eq!(body["output_config"]["effort"], "high");
}

#[test]
fn anthropic_handoff_maps_ids_uniquely_and_preserves_history() {
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
    let invalid = "functions.read:0";
    let hash = crate::util::hash(invalid.as_bytes());
    let reserved = format!("mc_{}", &hash[..61]);
    let ids = [
        invalid.to_string(),
        reserved.clone(),
        "call_x.y".to_string(),
        "z".repeat(80),
    ];
    let messages = vec![
        Message::user("question"),
        Message {
            role: "assistant".into(),
            content: ids
                .iter()
                .map(|id| Block::ToolUse {
                    input_error: None,
                    id: id.clone(),
                    name: "read".into(),
                    input: json!({}),
                })
                .collect(),
        },
        Message {
            role: "user".into(),
            content: ids
                .iter()
                .map(|id| Block::ToolResult {
                    tool_use_id: id.clone(),
                    content: "result".into(),
                    is_error: false,
                    images: vec![],
                })
                .collect(),
        },
    ];
    let before = messages.clone();
    let first = provider
        .payload("", &messages, &[], &ctx.attachments)
        .unwrap();
    let second = provider
        .payload("", &messages, &[], &ctx.attachments)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(messages, before);
    let calls = first["messages"][1]["content"].as_array().unwrap();
    let results = first["messages"][2]["content"].as_array().unwrap();
    let mapped = calls
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mapped[1], reserved);
    assert_eq!(
        mapped
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        ids.len()
    );
    for (id, result) in mapped.iter().zip(results) {
        assert!(
            id.len() <= 64
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        );
        assert_eq!(result["tool_use_id"], *id);
    }
}

#[test]
fn openai_handoff_downgrades_foreign_thinking_without_opaque_data() {
    let (_dir, ctx) = fixture();
    let provider = crate::provider::Provider::new(
        "fake",
        "off",
        &Configuration {
            path: ctx.workspace.join("config.json"),
        },
    )
    .unwrap();
    let messages = vec![
        Message::user("hi"),
        Message {
            role: "assistant".into(),
            content: vec![
                Block::Thinking {
                    thinking: "previous thought".into(),
                    signature: Some("secret-signature".into()),
                    source: "anthropic:other".into(),
                },
                Block::RedactedThinking {
                    data: "opaque-ciphertext".into(),
                    source: "anthropic:other".into(),
                },
                Block::text("answer"),
            ],
        },
    ];
    let body = provider
        .payload("", &messages, &[], &ctx.attachments)
        .unwrap();
    let text = body["messages"][1]["content"].as_str().unwrap();
    assert!(text.contains("previous thought") && text.contains("answer"));
    assert!(!body.to_string().contains("secret-signature"));
    assert!(!body.to_string().contains("opaque-ciphertext"));
}

#[test]
fn catalog_reasoning_tool_restrictions_prevent_invalid_chat_requests() {
    let (_dir, ctx) = fixture();
    let mut provider = crate::provider::Provider::new(
        "fake",
        "off",
        &Configuration {
            path: ctx.workspace.join("config.json"),
        },
    )
    .unwrap();
    provider.model = "gpt-5.6-sol".into();
    provider.parameter_compat =
        crate::configuration::catalog_model(&provider.model)["parameter_compat"].clone();
    let tools = [crate::models::ToolSpec {
        name: "read".into(),
        description: "read".into(),
        input_schema: json!({"type":"object"}),
        requires_approval: false,
    }];
    let messages = [Message::user("hello")];
    let body = provider
        .payload("", &messages, &tools, &ctx.attachments)
        .unwrap();
    assert_eq!(body["reasoning_effort"], "none");
    provider.effort = "max".into();
    assert!(
        provider
            .payload("", &messages, &tools, &ctx.attachments)
            .unwrap_err()
            .to_string()
            .contains("Responses")
    );
    assert_eq!(
        provider
            .payload("", &messages, &[], &ctx.attachments)
            .unwrap()["reasoning_effort"],
        "max"
    );
    provider.parameter_compat["reasoningWithTools"] = json!(true);
    assert_eq!(
        provider
            .payload("", &messages, &tools, &ctx.attachments)
            .unwrap()["reasoning_effort"],
        "max"
    );
    provider.model = "gpt-5.4-mini".into();
    provider.parameter_compat = serde_json::Value::Null;
    assert!(
        provider
            .payload("", &messages, &tools, &ctx.attachments)
            .is_err()
    );
}

#[test]
fn responses_only_model_rejects_chat_unless_gateway_overrides_it() {
    let (_dir, ctx) = fixture();
    let mut provider = crate::provider::Provider::new(
        "fake",
        "off",
        &Configuration {
            path: ctx.workspace.join("config.json"),
        },
    )
    .unwrap();
    let messages = [Message::user("hello")];
    for model in ["gpt-5-pro", "gpt-5.4-pro-2026-03-05", "o3-pro"] {
        provider.model = model.into();
        provider.effort = "low".into();
        assert!(
            provider
                .payload("", &messages, &[], &ctx.attachments)
                .unwrap_err()
                .to_string()
                .contains("Responses"),
            "{model}"
        );
    }
    provider.parameter_compat = json!({"chatSupported":true,"reasoningEffort":{"low":"high"}});
    assert_eq!(
        provider
            .payload("", &messages, &[], &ctx.attachments)
            .unwrap()["reasoning_effort"],
        "high"
    );
}

#[tokio::test]
async fn invalid_pending_write_recovers_as_failed_tool_result() {
    let mut h = Harness::new(json!({"turns":[]})).await;
    h.runtime.messages.push(Message {
        role: "assistant".into(),
        content: vec![Block::ToolUse {
            id: "invalid-pending".into(),
            name: "write".into(),
            input: json!("{\"path\":\"unsafe\","),
            input_error: Some("工具参数不是合法 JSON：unexpected end".into()),
        }],
    });
    h.runtime.prepare(&h.hooks).await.unwrap();
    assert!(!h.runtime.workspace.join("unsafe").exists());
    assert!(h.runtime.messages.iter().flat_map(|m| &m.content).any(|block|
        matches!(block, Block::ToolResult { tool_use_id, is_error: true, content, .. } if tool_use_id == "invalid-pending" && content.contains("合法 JSON"))));
    assert!(
        !h.event_types()
            .iter()
            .any(|kind| kind == "side_effect_unknown")
    );
}

fn x2_long_turn() -> Vec<Message> {
    let mut messages = vec![Message::user("original request and constraints")];
    for i in 0..10 {
        messages.push(Message {
            role: "assistant".into(),
            content: vec![
                Block::text(format!(
                    "decision {i}: {}",
                    "investigation details ".repeat(900)
                )),
                Block::ToolUse {
                    id: format!("split-{i}"),
                    name: "read".into(),
                    input: json!({"path":format!("src/{i}.rs")}),
                    input_error: None,
                },
            ],
        });
        messages.push(Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: format!("split-{i}"),
                content: "file contents ".repeat(300),
                is_error: false,
                images: vec![],
            }],
        });
    }
    messages.push(Message {
        role: "assistant".into(),
        content: vec![Block::text("latest progress must remain")],
    });
    messages
}

#[test]
fn x2_automatic_compaction_splits_a_single_turn_at_paired_boundaries() {
    let (_dir, ctx) = fixture();
    let mut messages = x2_long_turn();
    let before = context::estimate("", &messages, &[]);
    let stats = context::compact(
        &ctx.store,
        &ctx.session,
        "",
        &[],
        &mut messages,
        20_000,
        false,
    )
    .unwrap()
    .unwrap();
    assert!(
        stats["archived_units"].as_u64().unwrap() > 0,
        "a single turn must be archived, not just shrink tool results"
    );
    assert_eq!(messages[0].text(), "original request and constraints");
    assert_eq!(
        messages.last().unwrap().text(),
        "latest progress must remain"
    );
    assert!(context::estimate("", &messages, &[]) < before / 2);
    assert_eq!(
        context::paired_prefix(&messages, &[messages.len()]),
        Some(messages.len())
    );
    let summary = messages
        .iter()
        .find(|m| m.text().contains("此前历史已归档"))
        .unwrap()
        .text();
    let artifact = summary
        .split("[artifact:")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    let archived: Vec<Message> = serde_json::from_str(
        &fs::read_to_string(ctx.store.artifact_path(&ctx.session, artifact).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        context::paired_prefix(&archived, &[archived.len()]),
        Some(archived.len())
    );
}

#[test]
fn x2_large_completed_tool_inputs_and_old_assistant_text_have_artifact_refs() {
    let (_dir, ctx) = fixture();
    let input = json!({"path":"large.rs", "content":"代码正文".repeat(10_000)});
    let mut messages = vec![
        Message::user("constraints"),
        Message {
            role: "assistant".into(),
            content: vec![
                Block::text("long explanation ".repeat(3000)),
                Block::ToolUse {
                    id: "write-big".into(),
                    name: "write".into(),
                    input: input.clone(),
                    input_error: None,
                },
                Block::ToolUse {
                    id: "pending".into(),
                    name: "write".into(),
                    input: input.clone(),
                    input_error: None,
                },
            ],
        },
        Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: "write-big".into(),
                content: "written".into(),
                is_error: false,
                images: vec![],
            }],
        },
        Message::user("next"),
    ];
    assert_eq!(
        context::offload_large(&ctx.store, &ctx.session, &mut messages).unwrap(),
        2
    );
    let Block::ToolUse {
        input: replay, id, ..
    } = &messages[1].content[1]
    else {
        panic!("missing call")
    };
    assert_eq!(id, "write-big");
    assert_eq!(replay["path"], "large.rs");
    let artifact = replay["_artifact"].as_str().unwrap();
    let restored: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ctx.store.artifact_path(&ctx.session, artifact).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(restored, input);
    assert!(messages[1].text().contains("[artifact:"));
    let Block::ToolUse { input: pending, .. } = &messages[1].content[2] else {
        panic!("missing pending call")
    };
    assert_eq!(pending, &input, "unexecuted calls must never be rewritten");
    assert_eq!(
        context::offload_large(&ctx.store, &ctx.session, &mut messages).unwrap(),
        0
    );
}

#[test]
fn x3_force_compacts_one_or_two_prompt_tasks_below_the_trigger() {
    for prompts in [1, 2] {
        let (_dir, ctx) = fixture();
        let mut messages = x2_long_turn();
        if prompts == 2 {
            messages.push(Message::user("latest request"));
        }
        let before = context::estimate("", &messages, &[]);
        assert!(before < 200_000);
        let stats = context::compact(
            &ctx.store,
            &ctx.session,
            "",
            &[],
            &mut messages,
            200_000,
            true,
        )
        .unwrap()
        .expect("force must work with fewer than three prompts");
        assert!(stats["tokens_after"].as_u64().unwrap() < before);
        assert_eq!(messages[0].text(), "original request and constraints");
        assert_eq!(
            context::paired_prefix(&messages, &[messages.len()]),
            Some(messages.len())
        );
    }
}

#[test]
fn x3_force_can_offload_a_single_large_answer_but_keeps_the_original_request() {
    let (_dir, ctx) = fixture();
    let original = "完整用户约束".repeat(4000);
    let mut messages = vec![
        Message::user(&original),
        Message {
            role: "assistant".into(),
            content: vec![Block::text("长任务进度".repeat(6000))],
        },
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
    assert_eq!(messages[0].text(), original);
    assert!(messages[1].text().contains("[artifact:"));
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
        .is_none()
    );
    let mut only_request = vec![Message::user(original)];
    assert!(
        context::compact(
            &ctx.store,
            &ctx.session,
            "",
            &[],
            &mut only_request,
            200_000,
            true
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn x4_cjk_estimates_cover_text_tool_inputs_and_schemas() {
    assert_eq!(context::tokens("中文测试"), 4);
    assert!(context::tokens("abc中文") >= 3);
    let call = Block::ToolUse {
        id: "cjk".into(),
        name: "write".into(),
        input: json!({"path":"中文.rs","content":"中文".repeat(1000)}),
        input_error: None,
    };
    assert!(context::block_tokens(&call) >= 2000);
    let spec = crate::models::ToolSpec {
        name: "工具".into(),
        description: "描述".repeat(1000),
        input_schema: json!({"type":"object"}),
        requires_approval: false,
    };
    assert!(context::estimate("", &[], &[spec]) >= 2000);
}

#[tokio::test]
async fn x4_usage_anchors_prompt_and_estimates_only_appended_content() {
    let (_dir, ctx) = fixture();
    let configuration = ctx.catalog.lock().await.configuration.clone();
    let mut provider = crate::provider::Provider::new("fake", "off", &configuration).unwrap();
    provider.style = "anthropic".into();
    provider.window = 12_000;
    provider
        .set_script(json!({"turns":[{"text":"ok","input_tokens":9000}]}))
        .unwrap();
    let sink: tools::Sink = Arc::new(|_| {});
    let mut messages = vec![Message::user("original")];
    provider
        .stream("system", &messages, &[], &ctx.attachments, &sink)
        .await
        .unwrap();
    assert_eq!(
        provider
            .payload("system", &messages, &[], &ctx.attachments)
            .unwrap()["max_tokens"],
        1976
    );
    messages.push(Message::user("中".repeat(100)));
    assert_eq!(
        provider
            .payload("system", &messages, &[], &ctx.attachments)
            .unwrap()["max_tokens"],
        1776
    );
    messages[0] = Message::user("rewritten history invalidates the anchor");
    assert_eq!(
        provider
            .payload("system", &messages, &[], &ctx.attachments)
            .unwrap()["max_tokens"],
        8192
    );
}

#[tokio::test]
async fn x4_overflow_compacts_and_retries_the_same_round_once() {
    for repeated in [false, true] {
        let error =
            json!({"error":"provider HTTP 400 {\"error\":{\"code\":\"context_length_exceeded\"}}"});
        let mut h = Harness::new(json!({"turns":[error.clone(), if repeated {error} else {json!({"text":"recovered"})}, {"text":"must not retry twice"}]})).await;
        h.runtime.messages = vec![
            Message::user("original constraints"),
            Message {
                role: "assistant".into(),
                content: vec![Block::text("old progress ".repeat(3000))],
            },
        ];
        h.runtime
            .store
            .replace_messages(&h.runtime.session, &h.runtime.messages)
            .unwrap();
        h.runtime.message_revision = Some(
            h.runtime
                .store
                .message_revision(&h.runtime.session)
                .unwrap(),
        );
        let result = h.run(Some("continue")).await.unwrap();
        assert_eq!(
            result.exit_reason,
            if repeated {
                "provider_error"
            } else {
                "completed"
            }
        );
        assert_eq!(h.runtime.provider.index, 2);
        assert_eq!(h.runtime.rounds, 1);
        assert_eq!(
            h.event_types()
                .iter()
                .filter(|t| *t == "provider_retry")
                .count(),
            1
        );
        assert_eq!(h.runtime.messages[0].text(), "original constraints");
    }
}

#[tokio::test]
async fn x4_chinese_replay_continues_beyond_twice_the_window() {
    let source =
        "已改 agent/context.rs；关键决策是兼容旧会话；测试已通过，下一步继续验证。".repeat(180);
    let summary = "## Goal\n完成任务\n## Constraints & Preferences\n兼容旧会话\n## Progress\n### Done\n已改 agent/context.rs，测试通过\n### In Progress\n验证\n### Blocked\n无\n## Key Decisions\n兼容旧会话\n## Next Steps\n继续验证\n## Critical Context\n### Read Files\nagent/tests.rs\n### Modified Files\nagent/context.rs";
    let turns = (0..12).map(|_| json!({"text":source})).collect::<Vec<_>>();
    let mut h = Harness::new(json!({"turns":turns,"summary":summary})).await;
    h.runtime.provider.window = 30_000;
    h.runtime.provider.fake.as_mut().unwrap()["context_window"] = json!(30_000);
    assert!(source.chars().count() as u64 * 12 > h.runtime.provider.window * 2);
    for i in 0..12 {
        let result = h.run(Some(&format!("继续任务 {i}"))).await.unwrap();
        assert_eq!(
            result.exit_reason, "completed",
            "round {i}: {:?}",
            result.error
        );
    }
    assert!(
        h.runtime
            .messages
            .iter()
            .any(|m| m.text().contains(summary))
    );
    assert!(h.event_types().contains(&"context_compacted".into()));
    assert_eq!(h.runtime.provider.index, 12);
}

#[test]
fn x5_progressive_shrink_preserves_relevant_reads_and_stops_at_budget() {
    let (_dir, ctx) = fixture();
    let mut messages = vec![Message::user("continue work on src/keep.rs")];
    for i in 0..6 {
        let path = if i == 0 {
            "src/keep.rs".to_string()
        } else {
            format!("src/old-{i}.rs")
        };
        messages.push(Message {
            role: "assistant".into(),
            content: vec![Block::ToolUse {
                id: format!("read-{i}"),
                name: "read".into(),
                input: json!({"path":path}),
                input_error: None,
            }],
        });
        messages.push(Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: format!("read-{i}"),
                content: "abc".repeat(1000),
                is_error: false,
                images: vec![],
            }],
        });
    }
    let system = "x".repeat(9000);
    let before = context::estimate(&system, &messages, &[]);
    let stats = context::compact(
        &ctx.store,
        &ctx.session,
        &system,
        &[],
        &mut messages,
        9300,
        false,
    )
    .unwrap()
    .unwrap();
    let result = |id: &str| {
        messages
            .iter()
            .flat_map(|m| &m.content)
            .find_map(|b| match b {
                Block::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } if tool_use_id == id => Some(content.as_str()),
                _ => None,
            })
    };
    assert_eq!(
        result("read-0"),
        Some("abc".repeat(1000).as_str()),
        "latest read of a file in the active request must stay available"
    );
    assert_eq!(result("read-4"), Some("abc".repeat(1000).as_str()));
    assert_eq!(result("read-5"), Some("abc".repeat(1000).as_str()));
    assert!(
        stats["shrunk_results"].as_u64().unwrap() < 4,
        "stop once enough space has been recovered"
    );
    assert!(stats["tokens_after"].as_u64().unwrap() < before);
    let preview = result("read-1").unwrap();
    assert!(preview.contains("src/old-1.rs"));
    assert!(
        preview.chars().count() > 1000,
        "first shrink stage retains a useful file preview"
    );
    let artifact = preview
        .split("[artifact:")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    assert_eq!(
        fs::read_to_string(ctx.store.artifact_path(&ctx.session, artifact).unwrap()).unwrap(),
        "abc".repeat(1000)
    );
    assert_eq!(
        context::paired_prefix(&messages, &[messages.len()]),
        Some(messages.len())
    );
}

#[test]
fn x5_archive_refs_include_shell_commands_and_grep_output() {
    let (_dir, ctx) = fixture();
    let mut messages = vec![
        Message::user("original"),
        Message {
            role: "assistant".into(),
            content: vec![Block::ToolUse {
                id: "shell".into(),
                name: "bash".into(),
                input: json!({"command":"rg TODO agent/context.rs agent/provider.rs"}),
                input_error: None,
            }],
        },
        Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: "shell".into(),
                content: "agent/context.rs:12:TODO\n".repeat(300),
                is_error: false,
                images: vec![],
            }],
        },
        Message {
            role: "assistant".into(),
            content: vec![Block::ToolUse {
                id: "search".into(),
                name: "grep".into(),
                input: json!({"path":"src","pattern":"TODO"}),
                input_error: None,
            }],
        },
        Message {
            role: "user".into(),
            content: vec![Block::ToolResult {
                tool_use_id: "search".into(),
                content: "src/runtime.rs:42:TODO\n".repeat(300),
                is_error: false,
                images: vec![],
            }],
        },
        Message::user("latest"),
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
    let summary = messages.iter().map(Message::text).collect::<String>();
    assert!(
        summary.contains("agent/provider.rs"),
        "paths embedded in shell arguments must be retained"
    );
    assert!(
        summary.contains("src/runtime.rs"),
        "grep result filenames must be retained"
    );
}

#[tokio::test]
async fn p12_reopened_runtime_restores_model_calibration() {
    let mut h = Harness::new(json!({"turns":[{"text":"ready","input_tokens":20000}]})).await;
    assert_eq!(
        h.run(Some("inspect")).await.unwrap().exit_reason,
        "completed"
    );
    let scale = h.runtime.provider.prompt_scale;
    assert!(scale > 1.0);
    h.reopen().await.unwrap();
    assert_eq!(h.runtime.provider.prompt_scale, scale);
}

#[test]
fn rn_acceptance_failure_is_a_system_reminder() {
    let report = json!({"items":[{"item_id":"check","passed":false,"detail":"missing file"}]});
    let reminder = crate::goals::Goal::failure(&report);
    assert!(reminder.starts_with("<system-reminder>\n"));
    assert!(reminder.ends_with("\n</system-reminder>"));
    assert!(reminder.contains("missing file"));
}

#[tokio::test]
async fn r22_cancel_backfills_unstarted_calls_before_resume() {
    let mut h = Harness::new(json!({"turns":[{"tool_calls":[
        {"name":"write","arguments":{"path":"first.txt","content":"first"}},
        {"name":"write","arguments":{"path":"second.txt","content":"second"}}
    ]},{"text":"continued"}]}))
    .await;
    let cancel = h.hooks.cancel.clone();
    let sink = h.hooks.sink.clone();
    h.hooks.sink = Arc::new(move |v| {
        if v["item"]["type"] == "tool_call_result" {
            cancel.cancel();
        }
        sink(v);
    });
    let result = h
        .runtime
        .run(Some("write both".into()), vec![], h.hooks.clone())
        .await
        .unwrap();
    assert_eq!(result.exit_reason, "cancelled");
    assert!(h.runtime.workspace.join("first.txt").exists());
    assert!(!h.runtime.workspace.join("second.txt").exists());
    assert!(h.runtime.messages.iter().flat_map(|m| &m.content).any(
        |b| matches!(b, Block::ToolResult{content,is_error:true,..} if content.contains("未执行"))
    ));
    h.reopen().await.unwrap();
    assert_eq!(h.run(None).await.unwrap().exit_reason, "completed");
    assert!(!h.event_types().contains(&"side_effect_unknown".into()));
}

#[tokio::test]
async fn r22_recovery_distinguishes_queued_from_started() {
    let mut h = Harness::new(json!({"turns":[{"text":"continued"}]})).await;
    let message = Message {
        role: "assistant".into(),
        content: vec![
            Block::ToolUse {
                id: "queued".into(),
                name: "write".into(),
                input: json!({"path":"queued.txt","content":"x"}),
                input_error: None,
            },
            Block::ToolUse {
                id: "started".into(),
                name: "write".into(),
                input: json!({"path":"started.txt","content":"x"}),
                input_error: None,
            },
        ],
    };
    h.runtime
        .store
        .checkpoint(
            &h.runtime.session,
            "assistant_message",
            &json!({}),
            Some(&message),
            None,
            None,
        )
        .unwrap();
    h.runtime
        .store
        .event(
            &h.runtime.session,
            "tool_started",
            &json!({"call_id":"started"}),
        )
        .unwrap();
    h.reopen().await.unwrap();
    h.runtime.prepare(&h.hooks).await.unwrap();
    let results = h
        .runtime
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            Block::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert!(results["queued"].contains("未执行"));
    assert!(!results["queued"].contains("副作用状态未知"));
    assert!(results["started"].contains("副作用状态未知"));
    assert!(!h.runtime.workspace.join("queued.txt").exists());
}

#[tokio::test]
async fn r25_readonly_nested_instructions_succeed_without_an_extra_turn() {
    let mut h = Harness::new(json!({"turns":[{"tool_calls":[
        {"name":"read","arguments":{"path":"nested/file.txt"}},
        {"name":"ls","arguments":{"path":"nested"}},
        {"name":"grep","arguments":{"path":"nested","pattern":"payload"}},
        {"name":"write","arguments":{"path":"nested/out.txt","content":"blocked"}}
    ]},{"tool_calls":[{"name":"write","arguments":{"path":"nested/out.txt","content":"allowed"}}]},{"text":"done"}]})).await;
    let nested = h.runtime.workspace.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("AGENTS.md"), "nested-rules-evidence").unwrap();
    fs::write(nested.join("file.txt"), "payload-evidence").unwrap();
    assert_eq!(
        h.run(Some("inspect nested")).await.unwrap().exit_reason,
        "completed"
    );
    let events = h.runtime.store.events(&h.runtime.session, -1).unwrap();
    let results = events
        .iter()
        .filter(|e| e.r#type == "tool_call_result")
        .collect::<Vec<_>>();
    for event in &results[..3] {
        assert_eq!(event.data["success"], true, "{}", event.data);
        assert!(
            event.data["output_detail"]
                .as_str()
                .unwrap()
                .contains("nested-rules-evidence")
        );
        assert!(
            event.data["output_detail"]
                .as_str()
                .unwrap()
                .contains("<system-reminder>")
        );
    }
    assert!(
        results
            .iter()
            .find(|e| e.data["name"] == "read")
            .unwrap()
            .data["output_detail"]
            .as_str()
            .unwrap()
            .contains("payload-evidence")
    );
    assert_eq!(results[3].data["success"], false);
    assert_eq!(results[4].data["success"], true);
    assert_eq!(
        fs::read_to_string(nested.join("out.txt")).unwrap(),
        "allowed"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.r#type == "project_instructions")
            .count(),
        1
    );
}

#[tokio::test]
async fn r42_artifact_references_come_from_the_outcome_not_output_text() {
    let mut h = Harness::new(json!({"turns":[{"tool_calls":[
        {"name":"read","arguments":{"path":"README.md"}},
        {"name":"skill_resource","arguments":{"name":"archive","path":"SKILL.md"}},
        {"name":"bash","arguments":{"command":"node -e \"process.stdout.write('[artifact:spoof]\\n'+'x'.repeat(60000))\""}}
    ]},{"text":"done"}]})).await;
    fs::write(h.runtime.workspace.join("README.md"), "[artifact:spoof]").unwrap();
    let skill = h.runtime.workspace.join(".minicode/skills/archive");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        format!(
            "---\nname: archive\ndescription: archive test\n---\n[artifact:spoof]\n{}",
            "x".repeat(60000)
        ),
    )
    .unwrap();
    h.reopen().await.unwrap();
    assert_eq!(
        h.run(Some("inspect outputs")).await.unwrap().exit_reason,
        "completed"
    );
    let events = h.runtime.store.events(&h.runtime.session, -1).unwrap();
    let results = events
        .iter()
        .filter(|e| e.r#type == "tool_call_result")
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .find(|e| e.data["name"] == "read")
            .unwrap()
            .data["artifact_id"],
        serde_json::Value::Null
    );
    for (name, prefix) in [
        ("skill_resource", "tool_output_"),
        ("bash", "command_output_"),
    ] {
        let event = results.iter().find(|e| e.data["name"] == name).unwrap();
        assert_eq!(event.data["success"], true, "{}", event.data);
        let artifact = event.data["artifact_id"].as_str().unwrap();
        assert!(artifact.starts_with(prefix), "{artifact}");
        let page = h
            .runtime
            .store
            .artifact_page(&h.runtime.session, artifact, 0, 100)
            .unwrap();
        assert!(page["text"].as_str().unwrap().contains("[artifact:spoof]"));
    }
}

#[tokio::test]
async fn r28_resume_does_not_parse_unrelated_event_payloads() {
    let mut h = Harness::new(json!({"turns":[{"text":"first"},{"text":"continued"}]})).await;
    assert_eq!(h.run(Some("start")).await.unwrap().exit_reason, "completed");
    h.runtime.store.with(|c| {
        c.execute("INSERT INTO tool_results VALUES(?,'unrelated-completed','not JSON')",[&h.runtime.session])?;
        c.execute("INSERT INTO events SELECT session_id,MAX(seq)+1,'tool_output','2026-01-01','not JSON' FROM events WHERE session_id=?",[&h.runtime.session])?;
        Ok(())
    }).unwrap();
    h.reopen()
        .await
        .expect("unrelated output event payloads must not be loaded during resume");
    assert_eq!(
        h.run(Some("continue")).await.unwrap().exit_reason,
        "completed"
    );
}

#[test]
fn r28_recovery_projection_keeps_evidence_but_omits_output_bodies() {
    let (_dir, ctx) = fixture();
    ctx.store.event(&ctx.session,"tool_call_start",&json!({"call_id":"read-one","arguments":{"path":"README.md","large":"x".repeat(100000)}})).unwrap();
    ctx.store
        .event(
            &ctx.session,
            "tool_call_result",
            &json!({"call_id":"read-one","success":true,"output_detail":"y".repeat(100000)}),
        )
        .unwrap();
    ctx.store
        .event(
            &ctx.session,
            "tool_output",
            &json!({"text":"z".repeat(100000)}),
        )
        .unwrap();
    let events = ctx.store.recovery_events(&ctx.session).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data["arguments"]["path"], "README.md");
    assert_eq!(events[1].data["success"], true);
    assert!(serde_json::to_string(&events).unwrap().len() < 1000);
}

#[test]
fn s1_cached_title_survives_context_replacement_and_custom_rename() {
    let (_dir, ctx) = fixture();
    ctx.store
        .append(&ctx.session, &Message::user("原始任务标题\n详细要求"))
        .unwrap();
    ctx.store
        .replace_messages(&ctx.session, &[Message::user("compressed history")])
        .unwrap();
    assert_eq!(ctx.store.sessions().unwrap()[0]["title"], "原始任务标题");
    ctx.store.rename(&ctx.session, Some("custom")).unwrap();
    assert_eq!(ctx.store.sessions().unwrap()[0]["title"], "custom");
    ctx.store.rename(&ctx.session, None).unwrap();
    assert_eq!(ctx.store.sessions().unwrap()[0]["title"], "原始任务标题");
}

#[test]
fn s1_legacy_titles_are_migrated_and_1000_sessions_need_no_message_reads() {
    let (dir, ctx) = fixture();
    ctx.store.transaction(|c| {
        let columns = c.prepare("PRAGMA table_info(sessions)")?.query_map([],|r|r.get::<_,String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if columns.iter().any(|name| name=="auto_title") { c.execute_batch("ALTER TABLE sessions DROP COLUMN auto_title")?; }
        for i in 0..1000 {
            let id = format!("bulk_{i:04}");
            c.execute("INSERT INTO sessions(session_id,created_at,workspace,provider,model) VALUES(?,?,?,?,?)",rusqlite::params![id,format!("{i:020}"),"repo","fake","fake"])?;
            c.execute("INSERT INTO messages VALUES(?,0,'user',?)",rusqlite::params![id,serde_json::to_string(&vec![Block::text(format!("title {i}\nbody"))])?])?;
        }
        Ok(())
    }).unwrap();
    drop(ctx);
    let store = Store::open(dir.path().join("test.db")).unwrap();
    store.with(|c| { c.execute_batch("ALTER TABLE messages RENAME TO saved_messages; CREATE VIEW messages AS SELECT session_id,seq,role,json('invalid JSON') AS content FROM saved_messages")?; Ok(()) }).unwrap();
    let started = std::time::Instant::now();
    let rows = store
        .sessions()
        .expect("listing must read cached metadata, not message JSON");
    let elapsed = started.elapsed();
    eprintln!("S1: loaded {} sessions in {elapsed:?}", rows.len());
    assert_eq!(rows.len(), 1001);
    assert!(
        rows.iter()
            .any(|r| r["session_id"] == "bulk_0000" && r["title"] == "title 0")
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "{elapsed:?}"
    );
}

#[test]
fn s2_reads_continue_during_a_writer_transaction() {
    let (_dir, ctx) = fixture();
    let store = ctx.store.clone();
    let session = ctx.session.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        store.transaction(move |c| {
            c.execute(
                "UPDATE sessions SET status='paused' WHERE session_id=?",
                [session],
            )?;
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(std::time::Duration::from_millis(500));
            Ok(())
        })
    });
    ready_rx.recv().unwrap();
    let started = std::time::Instant::now();
    let before = ctx.store.session(&ctx.session).unwrap().unwrap();
    let elapsed = started.elapsed();
    let _ = release_tx.send(());
    writer.join().unwrap().unwrap();
    assert_eq!(
        before["status"], "running",
        "reader must see the last committed WAL snapshot"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(200),
        "{elapsed:?}"
    );
    assert_eq!(
        ctx.store.session(&ctx.session).unwrap().unwrap()["status"],
        "paused"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn s2_sqlite_busy_wait_does_not_starve_the_async_worker() {
    let (_dir, ctx) = fixture();
    let path = ctx.store.path.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        ready_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        connection.execute_batch("COMMIT").unwrap();
    });
    ready_rx.recv().unwrap();
    let store = ctx.store.clone();
    let session = ctx.session.clone();
    let started = std::time::Instant::now();
    let write = tokio::spawn(async move { store.event(&session, "probe", &json!({})) });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let responsive = started.elapsed() < std::time::Duration::from_millis(200);
    write.await.unwrap().unwrap();
    holder.join().unwrap();
    assert!(
        responsive,
        "SQLite waits must leave the runtime able to poll other tasks"
    );
}

#[test]
fn s4_parent_deletion_cascades_descendants_artifacts_and_locks() {
    let (_dir, ctx) = fixture();
    let child = ctx.store.create("repo", "fake", "fake").unwrap();
    let grandchild = ctx.store.create("repo", "fake", "fake").unwrap();
    let unrelated = ctx.store.create("repo", "fake", "fake").unwrap();
    for (parent, descendant) in [
        (&ctx.session, &child),
        (&child, &grandchild),
        (&grandchild, &ctx.session),
    ] {
        ctx.store
            .event(
                parent,
                "subagent_start",
                &json!({"child_session_id":descendant}),
            )
            .unwrap();
    }
    let ids = [&ctx.session, &child, &grandchild];
    for id in ids {
        drop(ctx.store.activation(id).unwrap());
        ctx.store.append(id, &Message::user("child task")).unwrap();
        ctx.store.spill(id, "history", "archived context").unwrap();
        ctx.store.freeze_prompt(id, "frozen", &json!([])).unwrap();
        ctx.store
            .event(id, "tool_started", &json!({"call_id":"pending"}))
            .unwrap();
    }
    ctx.store.delete(&ctx.session).unwrap();
    for id in ids {
        assert!(
            ctx.store.session(id).unwrap().is_none(),
            "{id} must be deleted"
        );
        assert!(!ctx.store.root().join("artifacts").join(id).exists());
        assert!(
            !ctx.store
                .root()
                .join(".test.db.locks")
                .join(format!("{id}.lock"))
                .exists()
        );
        assert!(ctx.store.messages(id).unwrap().is_empty());
        assert!(ctx.store.tool_execution(id).unwrap().is_empty());
        assert!(ctx.store.frozen_prompt(id).unwrap().is_none());
    }
    assert!(ctx.store.require(&unrelated).is_ok());
}

#[test]
fn s4_active_descendant_blocks_all_deletion_and_deleted_ids_leave_no_lock() {
    let (_dir, ctx) = fixture();
    let child = ctx.store.create("repo", "fake", "fake").unwrap();
    ctx.store
        .event(
            &ctx.session,
            "subagent_start",
            &json!({"child_session_id":child}),
        )
        .unwrap();
    let active = ctx.store.activation(&child).unwrap();
    assert!(ctx.store.delete(&ctx.session).is_err());
    assert!(ctx.store.require(&ctx.session).is_ok());
    assert!(ctx.store.require(&child).is_ok());
    drop(active);
    ctx.store.delete(&ctx.session).unwrap();
    assert!(ctx.store.activation(&ctx.session).is_err());
    assert!(
        !ctx.store
            .root()
            .join(".test.db.locks")
            .join(format!("{}.lock", ctx.session))
            .exists()
    );
}

#[test]
fn s5_incomplete_utf8_at_eof_is_one_replacement_with_stable_page_offsets() {
    let (_dir, ctx) = fixture();
    for prefix in ["".to_string(), "中文".to_string(), "a".repeat(8191)] {
        for tail in [vec![0xe4], vec![0xe4, 0xb8], vec![0xf0, 0x9f, 0x98]] {
            let artifact = ctx
                .store
                .spill(&ctx.session, "history", "placeholder")
                .unwrap();
            let path = ctx.store.artifact_path(&ctx.session, &artifact).unwrap();
            fs::write(&path, [prefix.as_bytes(), &tail].concat()).unwrap();
            let count = prefix.chars().count();
            let page = ctx
                .store
                .artifact_page(&ctx.session, &artifact, 0, count)
                .unwrap();
            assert_eq!(page["text"], prefix);
            assert_eq!(page["hasMore"], true);
            let empty = ctx
                .store
                .artifact_page(&ctx.session, &artifact, count, 0)
                .unwrap();
            assert_eq!(empty["text"], "");
            assert_eq!(empty["hasMore"], true);
            let last = ctx
                .store
                .artifact_page(&ctx.session, &artifact, count, 1)
                .unwrap();
            assert_eq!(last["text"], "\u{fffd}");
            assert_eq!(last["hasMore"], false);
            assert_eq!(last["total"], count + 1);
            let after = ctx
                .store
                .artifact_page(&ctx.session, &artifact, count + 1, 1)
                .unwrap();
            assert_eq!(after["text"], "");
            assert_eq!(after["total"], count + 1);
            let lines =
                crate::paging::artifact_lines(&path, &json!({"start_line":1,"limit":20000}))
                    .unwrap();
            assert_eq!(lines, format!("1\t{prefix}\u{fffd}"));
        }
    }
}

#[test]
fn s5_invalid_middle_bytes_still_fail_and_valid_split_scalars_are_unchanged() {
    let (_dir, ctx) = fixture();
    let artifact = ctx
        .store
        .spill(&ctx.session, "history", "placeholder")
        .unwrap();
    let path = ctx.store.artifact_path(&ctx.session, &artifact).unwrap();
    fs::write(&path, [b'a', 0xff, b'b']).unwrap();
    assert!(
        ctx.store
            .artifact_page(&ctx.session, &artifact, 0, 10)
            .is_err()
    );
    assert!(crate::paging::artifact_lines(&path, &json!({"start_line":1})).is_err());
    let text = format!("{}😀", "a".repeat(8191));
    fs::write(&path, &text).unwrap();
    let page = ctx
        .store
        .artifact_page(&ctx.session, &artifact, 8191, 1)
        .unwrap();
    assert_eq!(page["text"], "😀");
    assert_eq!(page["hasMore"], false);
}

#[tokio::test]
async fn r28_parent_resume_keeps_progress_after_completed_child_is_deleted() {
    let mut h = Harness::new(json!({"turns":[
        {"tool_calls":[{"name":"delegate","arguments":{"kind":"explore","task":"inspect"}}]},
        {"text":"{\"summary\":\"ok\",\"findings\":[],\"evidence_refs\":[],\"unresolved\":[]}"},
        {"text":"parent continued"}
    ]}))
    .await;
    h.hooks.settings.lock().unwrap().budget.max_rounds = 2;
    assert_eq!(
        h.run(Some("delegate then continue"))
            .await
            .unwrap()
            .exit_reason,
        "max_rounds"
    );
    let child = h
        .runtime
        .store
        .events(&h.runtime.session, -1)
        .unwrap()
        .into_iter()
        .find(|e| e.r#type == "subagent_result")
        .unwrap();
    assert_eq!(child.data["exit_reason"], "completed");
    h.runtime
        .store
        .delete(child.data["child_session_id"].as_str().unwrap())
        .unwrap();
    h.reopen().await.unwrap();
    assert_eq!(
        h.runtime.provider.index, 2,
        "parent must recover progress without the removed child's events"
    );
    h.hooks.settings.lock().unwrap().budget.max_rounds = 0;
    assert_eq!(h.run(None).await.unwrap().exit_reason, "completed");
    assert_eq!(
        h.runtime.messages.last().unwrap().text(),
        "parent continued"
    );
}
