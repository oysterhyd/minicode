use minicode::{
    attachments::Attachments,
    configuration::Configuration,
    context,
    models::{Block, Message},
    provider::Provider,
    store::Store,
    tools,
};
use serde_json::{Value, json};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Instant,
};

struct Allocator;
static COUNT: AtomicBool = AtomicBool::new(false);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT.load(Ordering::Relaxed) {
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNT.load(Ordering::Relaxed) {
            BYTES.fetch_add(size, Ordering::Relaxed);
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn measure(iterations: usize, mut f: impl FnMut()) -> Value {
    f();
    f();
    let mut runs = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            f();
        }
        runs.push(start.elapsed().as_secs_f64() * 1e6 / iterations as f64);
    }
    BYTES.store(0, Ordering::Relaxed);
    ALLOCS.store(0, Ordering::Relaxed);
    COUNT.store(true, Ordering::Relaxed);
    f();
    COUNT.store(false, Ordering::Relaxed);
    let mut sorted = runs.clone();
    sorted.sort_by(f64::total_cmp);
    json!({"median_us":sorted[3],"runs_us":runs,"iterations":iterations,"allocated_bytes":BYTES.load(Ordering::Relaxed),"allocations":ALLOCS.load(Ordering::Relaxed)})
}

fn main() {
    let store = Store::open(":memory:").unwrap();
    let id = store.create("fixture", "fake", "fake").unwrap();
    let messages = (0..2000)
        .map(|i| Message::user(format!("message {i} 中文")))
        .collect::<Vec<_>>();
    let input = json!({"path":"fixture","content":"中文😀\n\t\"\\x".repeat(100000)});
    let context_messages = vec![Message {
        role: "assistant".into(),
        content: vec![Block::ToolUse {
            id: "call".into(),
            name: "write".into(),
            input,
        }],
    }];
    let counters = json!({"rounds":7,"input_tokens":12345,"output_tokens":1000,"cache_read_tokens":100,"cache_write_tokens":200,"usage_available":true,"status":"completed","exit_reason":"completed"});
    let mut results = json!({});
    results["context_json_800k_scalars"] = measure(20, || {
        black_box(context::estimate(
            "system",
            black_box(&context_messages),
            tools::builtins(),
        ));
    });
    results["replace_2000_messages"] = measure(1, || {
        store.replace_messages(&id, black_box(&messages)).unwrap()
    });
    assert_eq!(store.messages(&id).unwrap(), messages);
    results["update_8_counters"] =
        measure(200, || store.update(&id, black_box(&counters)).unwrap());
    assert_eq!(store.require(&id).unwrap()["rounds"], 7);
    let temp = tempfile::tempdir().unwrap();
    let attachments = Attachments {
        root: temp.path().join("attachments"),
    };
    let mut provider = Provider::new(
        "fake",
        "off",
        &Configuration {
            path: temp.path().join("config.json"),
        },
    )
    .unwrap();
    let payload_messages = (0..1000)
        .map(|_| Message::user("x".repeat(10000)))
        .collect::<Vec<_>>();
    for style in ["openai", "anthropic"] {
        provider.style = style.into();
        results[format!("payload_{style}_10mb")] = measure(4, || {
            let payload = provider
                .payload(
                    "system",
                    black_box(&payload_messages),
                    tools::builtins(),
                    &attachments,
                )
                .unwrap();
            assert_eq!(
                payload["messages"].as_array().unwrap().len(),
                1000 + usize::from(style == "openai")
            );
            black_box(payload);
        });
    }
    let record = json!({"version":minicode::VERSION,"platform":std::env::consts::OS,"measurements":results,"allocation_metric":"total requested heap bytes per operation, including reallocations; not peak RSS","database":"in-memory SQLite, same data on both revisions"});
    std::fs::write(
        std::env::args().nth(1).expect("output JSON path"),
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();
}
