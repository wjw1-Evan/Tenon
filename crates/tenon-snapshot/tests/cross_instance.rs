use tenon_snapshot::SnapshotStore;

#[test]
fn tree_written_by_instance_a_readable_by_instance_b() {
    let data = tempfile::tempdir().unwrap();
    let snaps = tempfile::tempdir().unwrap();
    let ws = data.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let a = SnapshotStore::open(snaps.path(), "p", &ws, 2).unwrap();
    std::fs::write(ws.join("f.txt"), "v1\n").unwrap();
    let t = a.snapshot().unwrap();
    std::fs::write(ws.join("f.txt"), "v2\n").unwrap();

    let b = SnapshotStore::open(snaps.path(), "p", &ws, 2).unwrap();
    b.restore(&t).unwrap();
    assert_eq!(std::fs::read_to_string(ws.join("f.txt")).unwrap(), "v1\n");
}

#[test]
fn restore_safety_snapshot_then_target() {
    let data = tempfile::tempdir().unwrap();
    let snaps = tempfile::tempdir().unwrap();
    let ws = data.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let a = SnapshotStore::open(snaps.path(), "p", &ws, 2).unwrap();
    std::fs::write(ws.join("f.txt"), "v1\n").unwrap();
    let t1 = a.snapshot().unwrap();
    std::fs::write(ws.join("f.txt"), "HALF\n").unwrap();
    let b = SnapshotStore::open(snaps.path(), "p", &ws, 2).unwrap();
    let _safety = b.snapshot().unwrap();
    b.restore(&t1).unwrap();
    assert_eq!(std::fs::read_to_string(ws.join("f.txt")).unwrap(), "v1\n");
}
