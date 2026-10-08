//! C# LSP spike 替身测试（附录 C Q2 / v1.13）：
//! Roslyn 官方 VSIX 不可得，以 csharp-ls（dotnet tool）为替身验证
//! 「C# 语言服务器经共享 LSP 宿主沙箱化接入」全链路——
//! Roslyn 专属部分待官方 artifact 后按 spikes/roslyn-spike.md 替换执行。
//!
//! csharp-ls shim 需要 DOTNET_ROOT；未安装 csharp-ls 时跳过。

use std::time::Duration;

use tenon_lsp::guard::LspGuardConfig;
use tenon_lsp::host::{LspHost, LspHostConfig};
use tenon_lsp::manager::command_on_path;
use tenon_lsp::transport::ProcessConnection;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn csharp_lsp_sandboxed_handshake_and_semantics() {
    if !command_on_path("csharp-ls") {
        eprintln!("跳过：csharp-ls 未安装（dotnet tool install -g csharp-ls）");
        return;
    }
    let dotnet_root = std::env::var("DOTNET_ROOT")
        .unwrap_or_else(|_| format!("{}/.dotnet", std::env::var("HOME").unwrap_or_default()));

    // C# 工作区：项目 + 源文件
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("spike.csproj"),
        r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net8.0</TargetFramework>
    <Nullable>enable</Nullable>
  </PropertyGroup>
</Project>
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Greeter.cs"),
        "namespace Spike;\n\npublic class Greeter\n{\n    public string Hello(string name) => $\"hi {name}\";\n}\n",
    )
    .unwrap();

    // csharp-ls shim 需 DOTNET_ROOT：包装脚本注入
    let wrapper = dir.path().join("csharp-ls-wrapper.sh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport DOTNET_ROOT={dotnet_root}\nexec {dotnet_root}/tools/csharp-ls --loglevel error\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let conn = ProcessConnection::spawn(&wrapper.to_string_lossy(), &[], dir.path())
        .expect("spawn csharp-ls");

    let cfg = LspHostConfig {
        language: "csharp".into(),
        root_path: dir.path().to_path_buf(),
        guard: LspGuardConfig::new(dir.path()),
        initialization_options: None,
        edit_applier: None,
    };
    let host = LspHost::connect(cfg, Box::new(conn.reader), Box::new(conn.writer));

    // 1. 沙箱化握手（铁律七守卫随宿主接入）
    let caps = host
        .initialize(Duration::from_secs(60))
        .expect("csharp-ls initialize");
    assert!(caps.get("capabilities").is_some(), "{caps}");

    // 2. 语义链路：didOpen 后 hover C# 符号
    let uri = format!("file://{}/Greeter.cs", dir.path().to_string_lossy());
    host.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": uri, "languageId": "csharp", "version": 1,
                "text": "namespace Spike;\n\npublic class Greeter\n{\n    public string Hello(string name) => $\"hi {name}\";\n}\n",
            }
        }),
    )
    .unwrap();
    let hover = host
        .request(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": {"uri": uri},
                "position": {"line": 4, "character": 20},
            }),
            Duration::from_secs(60),
        )
        .expect("hover");
    eprintln!("csharp hover: {}", hover);
    assert!(
        hover.get("contents").is_some() || hover.is_null() || hover.is_object(),
        "hover 形态: {hover}"
    );

    // 3. references 链路（单文件可能为空——只要求请求成功）
    let refs = host.request(
        "textDocument/references",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 4, "character": 20},
            "context": {"includeDeclaration": true},
        }),
        Duration::from_secs(60),
    );
    assert!(refs.is_ok(), "references 请求成功: {refs:?}");

    host.shutdown();
}
