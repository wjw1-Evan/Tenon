//! Linux 沙箱执行路径（设计方案 §12.3：namespace + seccomp）。
//!
//! 分层（与 codex 沙箱同思路，§3.1 研读）：
//! 1. **network namespace**（断网态）：`unshare(CLONE_NEWNET)` —— 新网络命名
//!    空间内无任何接口，网络天然不可达；
//! 2. **Landlock**（LSM 文件访问控制，ABI v1+）：默认拒绝，仅授予
//!    「全盘读 + 项目内/临时目录读写」——写限项目内；
//! 3. **seccomp BPF**（防御纵深）：拦截 inet/inet6 `socket()` 创建——
//!    进程树（含 hooks 子进程）无法再获得网络套接字。
//!
//! Landlock 在旧内核（<5.13）不可用 → 降级为仅 seccomp + namespace（尽力限制，
//! 与降级档语义一致）；所有失败不 panic，返回 Err 由调用方决定降级。

#![cfg(target_os = "linux")]

use std::io;
use std::os::unix::io::RawFd;
use std::path::Path;

// ---------- Landlock 常量（linux/landlock.h，ABI v1） ----------

const LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const LANDLOCK_ADD_RULE: libc::c_long = 445;
const LANDLOCK_RESTRICT_SELF: libc::c_long = 446;

const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;

const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 0;
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 2;
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 3;
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 4;
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 5;
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 6;
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 7;
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 8;
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 9;
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 10;
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 11;

const ACCESS_READ: u64 = LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;
const ACCESS_WRITE: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_CHAR
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SOCK
    | LANDLOCK_ACCESS_FS_MAKE_FIFO
    | LANDLOCK_ACCESS_FS_MAKE_BLOCK
    | LANDLOCK_ACCESS_FS_MAKE_SYM;

const PR_SET_NO_NEW_PRIVS: libc::c_int = 38;
const SECCOMP_SET_MODE_FILTER: libc::c_int = 1;
const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;

const AF_UNIX: u32 = 1;
const AF_INET: u32 = 2;
const AF_INET6: u32 = 10;
const SYS_SOCKET: u32 = 41;
const ENETDOWN: u32 = 100;

#[repr(C)]
#[derive(Default)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

#[repr(C)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: RawFd,
}

/// 子进程内应用沙箱（pre_exec 上下文调用；全部不 panic）。
pub struct LinuxSandbox {
    /// 断网态：network namespace + seccomp 拦截 inet socket。
    pub offline: bool,
    /// 写入白名单路径（项目根 + 临时目录 + /dev/null）。
    pub write_paths: Vec<std::path::PathBuf>,
}

impl LinuxSandbox {
    /// 在当前（子）进程应用全部沙箱层；返回首个致命错误（无 = 成功）。
    pub fn apply(&self) -> Result<(), io::Error> {
        if self.offline {
            // 层 1：network namespace（无接口 = 无网络）。受限环境（加固主机 /
            // 托管 CI 禁用非特权 userns，§12.3 v1.150）EPERM → 降级跳过：
            // 网络隔离由层 3 seccomp inet 过滤兜底，写限仍由层 2 Landlock 承担
            if unshare_network().is_err() {
                eprintln!(
                    "[tenon-sandbox] unshare(CLONE_NEWNET) 不可用，层 1 降级（seccomp 兜底断网）"
                );
            }
        }
        // 层 2：Landlock 写限（旧内核 ENOSYS → 尽力降级，不致命）
        let _ = apply_landlock(&self.write_paths);
        if self.offline {
            // 层 3：seccomp 拦截 inet socket（防御纵深，覆盖 hooks 子进程）
            apply_seccomp_network_filter()?;
        }
        Ok(())
    }
}

fn unshare_network() -> Result<(), io::Error> {
    let rc = unsafe { libc::unshare(libc::CLONE_NEWNET) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn landlock_create_ruleset(handled: u64) -> Result<RawFd, io::Error> {
    let mut attr = LandlockRulesetAttr {
        handled_access_fs: handled,
    };
    let fd = unsafe {
        libc::syscall(
            LANDLOCK_CREATE_RULESET,
            &mut attr as *mut LandlockRulesetAttr,
            std::mem::size_of::<LandlockRulesetAttr>(),
            0u32,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd as RawFd)
    }
}

fn landlock_add_path_rule(ruleset: RawFd, path: &Path, access: u64) -> Result<(), io::Error> {
    let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad path"))?;
    let parent_fd = unsafe { libc::open(c_path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if parent_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut rule = LandlockPathBeneathAttr {
        allowed_access: access,
        parent_fd,
    };
    let rc = unsafe {
        libc::syscall(
            LANDLOCK_ADD_RULE,
            ruleset,
            LANDLOCK_RULE_PATH_BENEATH,
            &mut rule as *mut LandlockPathBeneathAttr,
            0u32,
        )
    };
    let result = if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    unsafe { libc::close(parent_fd) };
    result
}

fn landlock_restrict(ruleset: RawFd) -> Result<(), io::Error> {
    // 先锁 NO_NEW_PRIVS（Landlock 前置要求）
    if unsafe { libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::syscall(LANDLOCK_RESTRICT_SELF, ruleset, 0u32) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// 全盘只读 + 白名单路径读写。
fn apply_landlock(write_paths: &[std::path::PathBuf]) -> Result<(), io::Error> {
    let ruleset = landlock_create_ruleset(ACCESS_READ | ACCESS_WRITE)?;
    // 全盘只读
    landlock_add_path_rule(ruleset, Path::new("/"), ACCESS_READ)?;
    // 白名单读写
    for path in write_paths {
        let _ = landlock_add_path_rule(ruleset, path, ACCESS_READ | ACCESS_WRITE);
    }
    let result = landlock_restrict(ruleset);
    unsafe { libc::close(ruleset) };
    result
}

// ---------- seccomp BPF：拦截 inet/inet6 socket ----------

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

const BPF_LD: u16 = 0x00;
const BPF_JMP: u16 = 0x05;
const BPF_RET: u16 = 0x06;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;

const BPF_RET_ALLOW: u32 = 0x7FFF_0000;
const BPF_RET_ERRNO_ENETDOWN: u32 = 0x0005_0000 | ENETDOWN;

// seccomp_data 偏移（x86_64）：nr=0, arch=4, args[0]=16
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;
const OFF_ARGS0: u32 = 16;

fn bpf_stmt(code: u16, k: u32) -> SockFilter {
    SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn bpf_jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// 拦截 `socket(AF_INET/AF_INET6, ...)`：返回 ENETDOWN（断网语义）。
/// AF_UNIX 放行（工具链本地 IPC 不受影响）。
///
/// 布局（14 条，idx0-13）：
/// ```text
/// 0 LD arch            5 JEQ AF_UNIX jt→9  jf→6
/// 1 JEQ X86_64 jf→13   6 JEQ AF_INET jt→8  jf→7
/// 2 LD nr              7 JEQ AF_INET6 jt→8 jf→13
/// 3 JEQ socket jf→13   8 RET ERRNO(ENETDOWN)
/// 4 LD args[0]         9-13 RET ALLOW
/// ```
fn network_filter() -> [SockFilter; 14] {
    [
        bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFF_ARCH),
        bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 0, 11),
        bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFF_NR),
        bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, SYS_SOCKET, 0, 9),
        bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFF_ARGS0),
        bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, AF_UNIX, 3, 0),
        bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, AF_INET, 1, 0),
        bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, AF_INET6, 0, 4),
        bpf_stmt(BPF_RET | BPF_K, BPF_RET_ERRNO_ENETDOWN),
        bpf_stmt(BPF_RET | BPF_K, BPF_RET_ALLOW),
        bpf_stmt(BPF_RET | BPF_K, BPF_RET_ALLOW),
        bpf_stmt(BPF_RET | BPF_K, BPF_RET_ALLOW),
        bpf_stmt(BPF_RET | BPF_K, BPF_RET_ALLOW),
        bpf_stmt(BPF_RET | BPF_K, BPF_RET_ALLOW),
    ]
}

fn apply_seccomp_network_filter() -> Result<(), io::Error> {
    let filter = network_filter();
    let fprog = SockFprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };
    if unsafe { libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            0u32,
            &fprog as *const SockFprog,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpf_filter_layout_targets_in_range() {
        let filter = network_filter();
        assert_eq!(filter.len(), 14);
        // 末条必须 RET
        assert_eq!(filter[13].code, BPF_RET | BPF_K);
        // jump 目标均在界内（idx+1+offset ≤ 13）
        for (i, f) in filter.iter().enumerate() {
            if f.code == BPF_JMP | BPF_JEQ | BPF_K {
                let t_true = i + 1 + f.jt as usize;
                let t_false = i + 1 + f.jf as usize;
                assert!(t_true <= 13 && t_false <= 13, "idx {i} 越界");
                assert_eq!(
                    filter[t_true].code,
                    BPF_RET | BPF_K,
                    "idx {i} jt 目标应 RET"
                );
                assert_eq!(
                    filter[t_false].code,
                    BPF_RET | BPF_K,
                    "idx {i} jf 目标应 RET"
                );
            }
        }
    }

    #[test]
    fn errno_action_encodes_enetdown() {
        assert_eq!(BPF_RET_ERRNO_ENETDOWN & 0xFFFF, ENETDOWN);
        assert_eq!(BPF_RET_ERRNO_ENETDOWN >> 16, 0x0005);
    }
}
