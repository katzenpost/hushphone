//! The seccomp filter, put on a child process of this test so that the test itself stays free:
//! each case runs again as a child with `SANDBOX_CASE` set, enters the sandbox and probes it.

#![cfg(target_os = "linux")]

use std::io::ErrorKind;
use std::net::{TcpListener, UdpSocket};
use std::os::unix::net::UnixListener;
use std::process::Command;

use hushphone::sandbox::{self, Net};

fn denied<T>(r: std::io::Result<T>) -> bool {
    r.is_err_and(|e| e.kind() == ErrorKind::PermissionDenied)
}

/// Run `case` in a child process and expect it to pass there.
fn in_child(case: &str) {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([case, "--exact", "--nocapture", "--test-threads=1"])
        .env("SANDBOX_CASE", case)
        .env_remove("HUSHPHONE_SANDBOX")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && text.contains("1 passed"), "{case} in the sandbox:\n{text}");
}

fn child() -> bool {
    std::env::var("SANDBOX_CASE").is_ok()
}

fn probe_common() {
    assert!(denied(Command::new("/bin/true").status()), "a program was started");
    let sock = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("sandbox-{}", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    assert!(UnixListener::bind(&sock).is_ok(), "no unix socket");
    let _ = std::fs::remove_file(&sock);
    assert!(denied(UdpSocket::bind("127.0.0.1:0")), "a UDP socket was opened");
}

#[test]
fn unix_only() {
    if !child() {
        return in_child("unix_only");
    }
    sandbox::enter(Net::Unix).unwrap();
    probe_common();
    assert!(denied(TcpListener::bind("127.0.0.1:0")), "a TCP socket was opened");
    assert!(denied(TcpListener::bind("[::1]:0")), "a TCP socket was opened over IPv6");
}

#[test]
fn tcp_to_the_daemon() {
    if !child() {
        return in_child("tcp_to_the_daemon");
    }
    sandbox::enter(Net::Tcp).unwrap();
    probe_common();
    assert!(TcpListener::bind("127.0.0.1:0").is_ok(), "no TCP socket");
}

#[test]
fn the_process_keeps_to_itself() {
    if !child() {
        return in_child("the_process_keeps_to_itself");
    }
    hushphone::harden::process();
    // SAFETY: plain queries of the process's own state.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0), 0, "the process can be dumped");
        let mut core = libc::rlimit { rlim_cur: 1, rlim_max: 1 };
        libc::getrlimit(libc::RLIMIT_CORE, &mut core);
        assert_eq!(core.rlim_cur, 0, "core dumps are on");
    }
    let f = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("harden-{}", std::process::id()));
    std::fs::write(&f, b"secret").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
    std::fs::remove_file(&f).unwrap();
    assert_eq!(mode, 0o600, "a new file is readable by others");
    // Another process of the same user cannot look into this one: /proc/<pid>/environ takes the
    // same permission as reading its memory or tracing it.
    let peek = format!("head -c1 /proc/{}/environ >/dev/null 2>&1; echo $?", std::process::id());
    let o = Command::new("sh").args(["-c", &peek]).output().unwrap();
    assert_ne!(String::from_utf8_lossy(&o.stdout).trim(), "0", "another process looked into this one");
}
