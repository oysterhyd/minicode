use minicode::configuration::Configuration;
use serde_json::json;
use std::{fs, sync::mpsc, time::Duration};

#[test]
fn stale_configuration_snapshot_cannot_overwrite_a_newer_save() {
    let dir = tempfile::tempdir().unwrap();
    let config = Configuration {
        path: dir.path().join("config.json"),
    };
    let mut first = config.read().unwrap();
    let mut stale = first.clone();
    first["localExtensions"] = json!({"retained":true});
    config.write(&first).unwrap();
    let bytes = fs::read(&config.path).unwrap();
    stale["toolTimeouts"] = json!({"read":30});
    assert!(
        config
            .write(&stale)
            .unwrap_err()
            .to_string()
            .contains("conflict")
    );
    assert_eq!(fs::read(&config.path).unwrap(), bytes);
}

#[test]
fn configuration_writer_waits_for_the_cross_process_settings_lock() {
    use fs2::FileExt;
    let dir = tempfile::tempdir().unwrap();
    let config = Configuration {
        path: dir.path().join("config.json"),
    };
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(config.path.with_extension("json.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        tx.send(config.write(&json!({"services":[],"agents":[]})))
            .unwrap();
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "write bypassed the file lock"
    );
    fs2::FileExt::unlock(&lock).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    worker.join().unwrap();
}

#[test]
fn failed_oversized_configuration_save_preserves_the_committed_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = Configuration {
        path: dir.path().join("config.json"),
    };
    config.write(&json!({"services":[],"agents":[]})).unwrap();
    let mut data = config.read().unwrap();
    let bytes = fs::read(&config.path).unwrap();
    data["tooLarge"] = json!("x".repeat(4_000_001));
    assert!(config.write(&data).is_err());
    assert_eq!(fs::read(&config.path).unwrap(), bytes);
}

#[test]
fn interrupted_or_rejected_transaction_keeps_committed_data_and_releases_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let config = Configuration {
        path: dir.path().join("config.json"),
    };
    config.write(&json!({"services":[],"agents":[]})).unwrap();
    let bytes = fs::read(&config.path).unwrap();
    assert!(
        config
            .update(|data| {
                data["defaultModel"] = json!("uncommitted");
                anyhow::bail!("reject change")
            })
            .is_err()
    );
    assert_eq!(fs::read(&config.path).unwrap(), bytes);
    let snapshot = std::panic::catch_unwind(|| {
        let _ = config.update(|data| {
            data["defaultModel"] = json!("panicked");
            panic!("interrupted")
        });
    });
    assert!(snapshot.is_err());
    assert_eq!(fs::read(&config.path).unwrap(), bytes);
    // A stale temporary artifact is never promoted over the committed configuration.
    fs::write(dir.path().join(".tmp-uncommitted"), b"incomplete").unwrap();
    config
        .update(|data| {
            data["defaultModel"] = json!("committed");
            Ok(())
        })
        .unwrap();
    assert_eq!(config.read().unwrap()["defaultModel"], "committed");
}
