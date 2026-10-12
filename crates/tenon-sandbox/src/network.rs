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
    /// v2.22 白名单可配置（design.md §12.3）：host 合法性——非空、无 scheme /
    /// path / 端口、字符白名单（字母数字 . - _，小写归一）；内部镜像源常用
    /// 短横线与下划线子域。非法条目由调用方（settings 校验）整体拒绝。
    pub fn valid_registry_host(host: &str) -> bool {
        let h = host.trim();
        !h.is_empty()
            && h.len() <= 253
            && !h.contains("://")
            && !h.contains('/')
            && !h.contains(':')
            && h == h.to_lowercase()
            && h.split('.').all(|label| {
                !label.is_empty()
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
            && h.contains('.')
    }

    /// 内置基线 + settings 追加（并集，去重）——追加不替换：企业镜像源与官方
    /// 源并存，只放宽到用户显式声明的域。
    pub fn mirror_with_extra(extra: &[String]) -> Self {
        let Self::MirrorProxy {
            mut registries,
            proxy,
        } = Self::mirror_default()
        else {
            unreachable!("mirror_default 恒为 MirrorProxy")
        };
        for host in extra {
            let h = host.trim().to_lowercase();
            if Self::valid_registry_host(&h) {
                registries.insert(h);
            }
        }
        Self::MirrorProxy { registries, proxy }
    }

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

#[cfg(test)]
mod v222_registry_host_tests {
    use super::*;

    /// v2.22：host 校验矩阵 + 追加并集（内置基线保留）。
    #[test]
    fn registry_host_validation_and_union() {
        for ok in [
            "npm.company.internal",
            "mirrors.tuna.tsinghua.edu.cn",
            "registry.npmjs.org",
        ] {
            assert!(NetworkState::valid_registry_host(ok), "应合法: {ok}");
        }
        for bad in [
            "",
            "https://evil.com",
            "evil.com/path",
            "evil.com:8080",
            "EVIL.com",
            "evil.com..x",
            "-lead.internal",
            "trail-.internal",
            "spa ce.internal",
            "no-dot",
        ] {
            assert!(!NetworkState::valid_registry_host(bad), "应拒: {bad}");
        }
        let union = NetworkState::mirror_with_extra(&[
            "npm.company.internal".into(),
            "registry.npmjs.org".into(),
        ]);
        let NetworkState::MirrorProxy { registries, .. } = union else {
            unreachable!()
        };
        assert!(registries.contains("npm.company.internal"));
        assert!(registries.contains("registry.npmjs.org"), "内置基线保留");
    }
}
