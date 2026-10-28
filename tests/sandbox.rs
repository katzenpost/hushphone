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
