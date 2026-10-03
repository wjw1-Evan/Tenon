//! sqlite-vec 冒烟测试（附录 C Q3 spike，M0 交付）：
//! vec0 虚表建表 / 插入 / KNN 召回全链路。通过则确认 L4 向量层选型成立。

use rusqlite::Connection;

#[test]
fn vec0_virtual_table_smoke() {
    unsafe {
        type InitFn = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        let init: InitFn = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
        let rc = rusqlite::ffi::sqlite3_auto_extension(Some(init));
        assert_eq!(rc, rusqlite::ffi::SQLITE_OK, "auto extension 注册失败");
    }
    let conn = Connection::open_in_memory().unwrap();

    // 1. 建表（L4 嵌入维度在正式表中配置；此处 4 维便于手算）
    conn.execute_batch("CREATE VIRTUAL TABLE l4_chunks_vec USING vec0(embedding float[4]);")
        .expect("vec0 建表");

    // 2. 插入向量
    for (id, v) in [
        (1i64, [1.0f32, 0.0, 0.0, 0.0]),
        (2, [0.0, 1.0, 0.0, 0.0]),
        (3, [0.9, 0.1, 0.0, 0.0]),
    ] {
        let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        conn.execute(
            "INSERT INTO l4_chunks_vec(rowid, embedding) VALUES (?1, ?2)",
            rusqlite::params![id, blob],
        )
        .expect("插入向量");
    }

    // 3. KNN 查询（top-2 最邻近）
    let query: Vec<u8> = [1.0f32, 0.0, 0.0, 0.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let mut stmt = conn
        .prepare(
            "SELECT rowid, distance FROM l4_chunks_vec
             WHERE embedding MATCH ?1 ORDER BY distance LIMIT 2",
        )
        .unwrap();
    let rows: Vec<(i64, f32)> = stmt
        .query_map(rusqlite::params![query], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 1, "最邻近应为向量 1（L2 距离 0）");
    assert!(rows[0].1 < rows[1].1, "按距离升序");
}
