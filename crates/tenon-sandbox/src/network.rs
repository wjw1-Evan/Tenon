//! 网络三态（设计方案 §12.3）：断网 / 镜像代理 / 域名代理。

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkState {
    /// 断网：测试 / 构建 / 纯分析（run_tests / run_build）
    Offline,
    /// 镜像代理：仅预授权 registry（npm / pypi / nuget / crates…，B 级；
    /// install_deps）
    MirrorProxy { registries: BTreeSet<String> },
    /// 域名代理：直执请求并审计目标（C 级；http_fetch）
    DomainProxy { hosts: BTreeSet<String> },
}

impl NetworkState {
    pub fn offline() -> Self {
        Self::Offline
    }

    pub fn mirror_default() -> Self {
        Self::MirrorProxy {
            registries: [
                "registry.npmjs.org",
                "pypi.org",
                "files.pythonhosted.org",
                "crates.io",
                "static.crates.io",
                "nuget.org",
                "api.nuget.org",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }

    /// 该状态下是否允许访问某主机。
    pub fn allows_host(&self, host: &str) -> bool {
        match self {
            NetworkState::Offline => false,
            NetworkState::MirrorProxy { registries } => registries.contains(host),
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
