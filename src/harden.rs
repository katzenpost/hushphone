//! What the process does to itself before anything else: the files it creates are its owner's
//! alone, and it can neither be dumped nor be traced or read by other processes of the same
//! user. The keys of every group, and every message, live in this process and in its data
//! directory.

use std::path::Path;

/// Restrict the process: owner-only files, no core dumps, no ptrace or /proc/<pid>/mem from
/// the user's other processes.
pub fn process() {
    // SAFETY: plain system calls with constant arguments.
    unsafe {
        libc::umask(0o077);
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        libc::setrlimit(libc::RLIMIT_CORE, &none);
    }
}

/// Make `dir` and everything in it its owner's alone, as an older version may have left it
/// readable by others.
pub fn data_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return Ok(()) };
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        for e in std::fs::read_dir(dir)? {
            data_dir(&e?.path())?;
        }
    } else if meta.is_file() {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn an_old_data_directory_is_closed() {
        let d = crate::test_dir();
        let state = d.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(state.join("state.redb"), b"x").unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o775)).unwrap();
        std::fs::set_permissions(state.join("state.redb"), std::fs::Permissions::from_mode(0o664)).unwrap();
        data_dir(d.path()).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(d.path()), 0o700);
        assert_eq!(mode(&state), 0o700);
        assert_eq!(mode(&state.join("state.redb")), 0o600);
    }
}
