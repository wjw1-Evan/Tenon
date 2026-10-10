//! 网络三态（设计方案 §12.3）：断网 / 镜像代理 / 域名代理。

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkState {
    /// 断网：测试 / 构建 / 纯分析（run_tests / run_build）
    Offline,
    /// 镜像代理：仅预授权 registry（npm / pypi / nuget / crates…，B 级；
    /// install_deps）。`proxy` = v2.0 域过滤代理进程地址（design-v2.md §4.4）：
    /// Some = seatbelt 收紧为「deny network* + 仅回环代理」，白名单由代理强制；
    /// None = fail-closed 同样 deny network*（纵深防御——executor 层在代理
    /// 不可用时已先行拒绝，不静默回退全网放行）。
    MirrorProxy {
        registries: BTreeSet<String>,
        #[serde(default)]
        proxy: Option<std::net::SocketAddr>,
    },
    /// 域名代理：直执请求并审计目标（C 级；http_fetch）
    DomainProxy { hosts: BTreeSet<String> },
}

impl NetworkState {
    pub fn offline() -> Self {
        Self::Offline
    }

    pub fn mirror_default() -> Self {
        Self::MirrorProxy {
            // v2.0 扩充（design-v2.md §4.4）：覆盖 install_policy 白名单全部
            // 包管理器的官方源 + git 依赖 / SPM 所需的 GitHub 域——面扩大如实
            // 列出（GitHub 可托管用户内容，属「git 依赖可用性」的取舍）
            registries: [
                "registry.npmjs.org",
                "registry.yarnpkg.com",
                "pypi.org",
                "files.pythonhosted.org",
                "crates.io",
                "static.crates.io",
                "index.crates.io",
                "nuget.org",
                "api.nuget.org",
                "proxy.golang.org",
                "sum.golang.org",
                "repo1.maven.org",
                "rubygems.org",
                "repo.packagist.org",
                "hex.pm",
                "repo.hex.pm",
                "jsr.io",
                "repo.anaconda.com",
                "conda.anaconda.org",
                "github.com",
                "codeload.github.com",
                "api.github.com",
                "raw.githubusercontent.com",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            proxy: None,
        }
    }

    /// 该状态下是否允许访问某主机。
    pub fn allows_host(&self, host: &str) -> bool {
        match self {
            NetworkState::Offline => false,
            NetworkState::MirrorProxy { registries, .. } => registries.contains(host),
            NetworkState::DomainProxy { hosts } => hosts.contains(host),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            NetworkState::Offline => "offline",
            NetworkState::MirrorProxy { .. } => "mirror_proxy",
            NetworkState::DomainProxy { .. } => "domain_proxy",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_blocks_everything() {
        let n = NetworkState::offline();
        assert!(!n.allows_host("registry.npmjs.org"));
        assert!(!n.allows_host("example.com"));
        assert_eq!(n.label(), "offline");
    }

    #[test]
    fn mirror_proxy_allows_only_preauthorized_registries() {
        let n = NetworkState::mirror_default();
        assert!(n.allows_host("registry.npmjs.org"));
        assert!(n.allows_host("pypi.org"));
        assert!(n.allows_host("crates.io"));
        assert!(!n.allows_host("evil.example.com"));
        assert_eq!(n.label(), "mirror_proxy");
    }

    #[test]
    fn domain_proxy_metadata_preserved() {
        let n = NetworkState::DomainProxy {
            hosts: ["docs.rs".to_string()].into_iter().collect(),
        };
        assert!(n.allows_host("docs.rs"));
        assert!(!n.allows_host("registry.npmjs.org"));
        assert_eq!(n.label(), "domain_proxy");
    }
}
