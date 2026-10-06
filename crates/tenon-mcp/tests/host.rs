//! McpHost 集成测试（§13.5 v1.145）：多服务器懒 spawn、工具命名、分级策略、
//! 失败跳过与调用回显——经 fake-mcp-server 测试二进制走真 stdio 链路。

use std::collections::BTreeMap;
use tenon_mcp::{mcp_tool_name, parse_mcp_tool_name, McpHost, McpServerConfig};

fn fake_server_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fake-mcp-server")
}

fn host_with(configs: BTreeMap<String, McpServerConfig>) -> McpHost {
    McpHost::new(configs, std::env::temp_dir())
}

fn server_config(command: &str, net: bool) -> McpServerConfig {
    McpServerConfig {
        command: command.to_string(),
        args: vec![],
        env: BTreeMap::new(),
        enabled: true,
        permissions: if net { vec!["net:*".into()] } else { vec![] },
        source: Some("owner/repo".into()),
        version: Some("1.0.0".into()),
    }
}

#[test]
fn tool_name_roundtrip_is_unambiguous() {
    let name = mcp_tool_name("github", "create_issue");
    assert_eq!(name, "mcp_github_create_issue");
    let (server, tool) = parse_mcp_tool_name(&name).unwrap();
    assert_eq!(server, "github");
    assert_eq!(tool, "create_issue");
    assert!(parse_mcp_tool_name("read_file").is_none());
    assert!(parse_mcp_tool_name("mcp_").is_none());
    assert!(parse_mcp_tool_name("mcp_srv_").is_none());
}

#[test]
fn serde_defaults_match_settings_schema() {
    let cfg: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-github"]
    }))
    .unwrap();
    assert!(cfg.enabled, "enabled 缺省 true");
    assert!(cfg.env.is_empty());
    assert!(cfg.permissions.is_empty());
    assert_eq!(cfg.source, None);
}

#[test]
fn host_lists_and_calls_via_stdio() {
    let mut configs = BTreeMap::new();
    configs.insert("srv".to_string(), server_config(fake_server_bin(), false));
    let host = host_with(configs);
    assert!(!host.is_empty());

    let tools = host.list_tools();
    assert_eq!(tools.len(), 1, "fake 服务器提供 echo 工具");
    assert_eq!(tools[0].server, "srv");
    assert_eq!(tools[0].tool.name, "echo");

    let out = host
        .call("srv", "echo", serde_json::json!({"text": "你好"}))
        .unwrap();
    assert_eq!(out, "echo:echo:{\"text\":\"你好\"}");

    // 连接复用：第二次调用走缓存（无断言面，只验证不出错）。
    let out = host.call("srv", "echo", serde_json::json!({})).unwrap();
    assert!(out.starts_with("echo:echo:"));
}

#[test]
fn level_policy_net_server_tools_are_c_others_d() {
    let mut configs = BTreeMap::new();
    configs.insert("net".to_string(), server_config(fake_server_bin(), true));
    configs.insert("local".to_string(), server_config(fake_server_bin(), false));
    let host = host_with(configs);
    let tools = host.list_tools();
    assert_eq!(tools.len(), 2);

    let policy = host.level_policy_with(&tools);
    assert_eq!(
        policy.level_for(&mcp_tool_name("net", "echo")),
        tenon_mcp::McpToolLevel::C,
        "net:* 服务器工具 → C"
    );
    assert_eq!(
        policy.level_for(&mcp_tool_name("local", "echo")),
        tenon_mcp::McpToolLevel::D,
        "无 net:* 服务器工具 → D"
    );
    assert_eq!(
        policy.level_for("mcp_unknown_tool"),
        tenon_mcp::McpToolLevel::D,
        "未见过的 mcp_ 工具默认 D"
    );
}

#[test]
fn failing_server_is_skipped_and_disabled_ignored() {
    let mut configs = BTreeMap::new();
    configs.insert(
        "broken".to_string(),
        server_config("/nonexistent/tenon-no-such-bin", false),
    );
    let mut disabled = server_config(fake_server_bin(), false);
    disabled.enabled = false;
    configs.insert("off".to_string(), disabled);
    configs.insert("good".to_string(), server_config(fake_server_bin(), false));
    let host = host_with(configs);

    let tools = host.list_tools();
    assert_eq!(tools.len(), 1, "spawn 失败跳过 + disabled 不列");
    assert_eq!(tools[0].server, "good");
}

#[test]
fn empty_host_is_empty() {
    let host = host_with(BTreeMap::new());
    assert!(host.is_empty());
    assert!(host.list_tools().is_empty());
}
