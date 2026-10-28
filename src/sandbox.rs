//! A seccomp filter the process puts on itself once it knows how it reaches kpclientd. The daemon
//! does all the networking, so the app needs no IP socket when the daemon listens on a unix
//! socket, and only TCP when it listens on a port (kpclientd-android does, on 127.0.0.1). Nothing
//! the app does needs to start a program, trace one, or reach into the kernel's administration,
//! so those calls fail too: a flaw in a library the app uses cannot open a shell or a socket of
//! its own through them.

use std::collections::BTreeMap;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition as Cond, SeccompFilter,
    SeccompRule,
};

/// The sockets the app may open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Net {
    /// Unix sockets only: kpclientd at `@name` or a path, and the desktop's own sockets.
    Unix,
    /// Unix sockets and TCP: kpclientd at `host:port`. No UDP or raw sockets.
    Tcp,
}

impl Net {
    /// What reaching kpclientd at `address` needs.
    pub fn for_daemon(address: &str) -> Net {
        if address.starts_with('@') || address.starts_with('/') { Net::Unix } else { Net::Tcp }
    }

    /// Whether `self` allows what `other` needs.
    pub fn allows(self, other: Net) -> bool {
        self == Net::Tcp || other == Net::Unix
    }
}

/// Calls that fail whatever their arguments.
const DENIED: &[libc::c_long] = &[
    libc::SYS_execve,
    libc::SYS_execveat,
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_kcmp,
    libc::SYS_bpf,
    libc::SYS_perf_event_open,
    libc::SYS_userfaultfd,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_open_by_handle_at,
    libc::SYS_name_to_handle_at,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_kexec_load,
    libc::SYS_reboot,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_acct,
    libc::SYS_quotactl,
    libc::SYS_settimeofday,
    libc::SYS_clock_settime,
    libc::SYS_adjtimex,
    libc::SYS_fanotify_init,
];

fn rule(conds: Vec<Cond>) -> SeccompRule {
    SeccompRule::new(conds).expect("a seccomp rule")
}

fn arg(n: u8, op: Op, v: u64) -> Cond {
    Cond::new(n, Len::Dword, op, v).expect("a seccomp condition")
}

/// The rules for `socket`: those that match fail.
fn socket_rules(net: Net) -> Vec<SeccompRule> {
    let (unix, inet, inet6) = (libc::AF_UNIX as u64, libc::AF_INET as u64, libc::AF_INET6 as u64);
    match net {
        Net::Unix => vec![rule(vec![arg(0, Op::Ne, unix)])],
        Net::Tcp => {
            // Any family but unix and IP; and IP of any type but a stream.
            let mut r = vec![rule(vec![arg(0, Op::Ne, unix), arg(0, Op::Ne, inet), arg(0, Op::Ne, inet6)])];
            for family in [inet, inet6] {
                // The low four bits of the type are the kind; the rest are flags.
                for kind in (0..16).filter(|&k| k != libc::SOCK_STREAM as u64) {
                    r.push(rule(vec![arg(0, Op::Eq, family), arg(1, Op::MaskedEq(0xf), kind)]));
                }
            }
            r
        }
    }
}

fn program(net: Net) -> Result<BpfProgram, String> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = DENIED.iter().map(|&s| (s, Vec::new())).collect();
    rules.insert(libc::SYS_socket, socket_rules(net));
    let arch = std::env::consts::ARCH.try_into().map_err(|e| format!("{e}"))?;
    let filter = SeccompFilter::new(rules, SeccompAction::Allow, SeccompAction::Errno(libc::EPERM as u32), arch)
        .map_err(|e| e.to_string())?;
    BpfProgram::try_from(filter).map_err(|e| e.to_string())
}

/// Put the filter on every thread of the process, for good. `HUSHPHONE_SANDBOX=off` leaves it off,
/// for debugging.
pub fn enter(net: Net) -> Result<(), String> {
    if std::env::var("HUSHPHONE_SANDBOX").is_ok_and(|v| v == "off") {
        return Ok(());
    }
    let p = program(net)?;
    seccompiler::apply_filter_all_threads(&p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_address_decides() {
        assert_eq!(Net::for_daemon("@katzenpost"), Net::Unix);
        assert_eq!(Net::for_daemon("/run/kpclientd.sock"), Net::Unix);
        assert_eq!(Net::for_daemon("127.0.0.1:64331"), Net::Tcp);
        assert!(Net::Tcp.allows(Net::Unix) && !Net::Unix.allows(Net::Tcp));
    }

    #[test]
    fn both_programs_compile() {
        assert!(program(Net::Unix).is_ok());
        assert!(program(Net::Tcp).is_ok());
    }
}
