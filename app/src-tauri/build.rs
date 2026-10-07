fn main() {
    // v1.156：daemon 托管 UI（http://127.0.0.1）属 remote 上下文，壳自定义命令
    // 须经 app ACL 显式授权——此处生成 allow-* 权限，供 capabilities 引用。
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_handshake",
            "get_update_notes",
            "dismiss_update_notes",
            "get_update_ready",
            "install_update",
        ]),
    ))
    .expect("tauri-build 失败");
}
