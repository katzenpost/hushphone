# Security notes

This document covers four topics:
- what hushphone does to keep its user safe;
- where hushphone and the libraries it depends on leave safe Rust, and why;
- how they keep critical data (keys, capabilities, messages, voice clips) from leaking out of memory;
- how the interface is designed so that what the app does matches what the user meant.

Line numbers are those of the revisions pinned in `Cargo.toml`:
- pigeonhole_protocols `ad9d3e2`;
- rustic_audio `0be6248`.

## What hushphone does


hushphone follows Paw's
[GUI security guidelines](https://github.com/katzenpost/paw/blob/main/docs/GUI_SECURITY_GUIDELINES.md):
whatever other members send is untrusted.

- **Names.** A member's own name is shown as `~name`. Your own name for a member is shown in bold
  instead, and only to you. Names that look alike show 64 bits of the member's key, too many for
  a member to make a key that matches another's. Names and file names lose every character
  Unicode renders as nothing (bidi controls, joiners, variation selectors, fillers, tags), and
  runs of whitespace.
- **Content stays in its frame.** Messages are plain text, clipped to their bubble. Long words get
  places to break. The app's own notices use a frame that no message can draw.
- **Controls wait before acting.** A control takes no click until it has been still, and about
  the same message, for half a second, so a message that arrives and moves the layout, or a row
  that comes to show another message, cannot steer a click. A dangerous button in a confirmation
  waits 0.7 s, and the safe one has the focus.
- **Authority follows the user's action.** Only the window's own callbacks can copy, paste, open
  the microphone, pick or save a file; nothing that arrives from the network can. Received files
  are never saved on their own, and their type is read from their bytes, not their name. A
  recording indicator is shown for exactly as long as the microphone is open.
- **Files pass through one directory.** On Linux, hushphone reads only `~/hushphone/send` and
  writes only `~/hushphone/received`; its sandbox lets it see no other file of yours. The desktop's
  file chooser is not used, since it hands an app a path and leaves it to read the file with the
  user's whole authority. On Android, the system's document picker hands over one file at a time
  and nothing else; hushphone holds no storage permission.
- **Notices never take over.** Dialogs open only from your own action. Notices from the network
  never take the focus.
- **Encrypted at rest.** As Signal does with its database, hushphone keeps its state file
  encrypted whole, with no passphrase: the groups, their caps and every message are in one file,
  `state.sealed`, that is written out encrypted after every change. Each attachment and voice clip
  beside it is encrypted whole too. Both use XChaCha20-Poly1305 under a random key that the system
  keeps for hushphone. All that can be seen on disk is how many files there are, how large, and
  their names.
  - On Linux, the key is kept in the session's keyring (the Secret Service: GNOME Keyring or
    KWallet), which unlocks when you log in. With no keyring running, hushphone keeps its state
    unsealed and says so in a strip under the group list.
  - On Android, the key is wrapped by an AES key held in Android's Keystore, in StrongBox where the
    phone has one, and never leaves it.
  - A copy of the files, from a backup or a lost disk, cannot be read without that key. Someone
    who can run programs as you while you are logged in (or as hushphone, on an unlocked phone)
    can ask for the key, as hushphone does.
  - Voice clips are recorded and played in memory, never in files. A clip being recorded or
    encoded lives in guarded memory: between guard pages, locked out of swap where the system
    allows, left out of core dumps, and wiped when freed.
  - A file you open with another program has to be written out unsealed for it, in hushphone's
    cache; it is wiped the next time and when hushphone starts.
  - An older, unsealed state is sealed the first time hushphone starts with a key. The unsealed
    copy is written over and removed, but flash storage may still hold its old blocks.
- **On disk.** Settings (your name, the daemon's address, the theme, the wire version, and on Android the transport and replica) are written with mode 0600
  and are not sealed. The names you give members, with their keys, and what you have read are a
  contact list, so they are kept with the state, sealed when it is. A file you save is written
  out plain in `~/hushphone/received`, since that is what saving is for. Logs hold no message
  text.
- **Sandboxed.** On the mixnet, kpclientd does all the networking, so hushphone needs very little
  of the system.
  - **seccomp.** Once hushphone knows how it reaches Pigeonhole, it puts a seccomp filter on every
    thread. With kpclientd on a unix socket, as on Linux (`@katzenpost`), it may open unix
    sockets only, with no TCP or UDP at all. With kpclientd at `host:port`, or on a Reticulum
    transport, it may also open TCP sockets, but still no UDP or raw ones. In either case it cannot start a program,
    trace a process or read its memory, use io_uring, BPF or perf, load kernel modules, mount, or
    change namespaces. The filter cannot be lifted while the app runs, so a TCP address set in
    Settings takes effect after a restart. `HUSHPHONE_SANDBOX=off` leaves the filter off, for
    debugging.
  - **AppArmor.** The Debian package installs an AppArmor profile, `/etc/apparmor.d/hushphone`. It
    allows no IP and no programs; reads of `~/hushphone/send` and writes to `~/hushphone/received`
    and hushphone's own data and cache, and nothing else of yours; the daemon's socket,
    `@katzenpost`, the display and the sound server; and on the session bus only the bus itself and
    the keyring, so that neither the user's systemd nor the portals can be asked to run or open
    anything. To reach kpclientd over TCP or the mesh, or kpclientd at another
    abstract socket, add the rule the profile
    names in its comment to `/etc/apparmor.d/local/hushphone`.
  - **If the filter cannot be applied,** hushphone does not connect; it says so, and
    `HUSHPHONE_SANDBOX=off` is the way to run without the filter on purpose.
  - **Android.** On the mixnet, hushphone gets its connections to kpclientd from
    kpclientd-android, as unix sockets, so the unix-only filter applies there too, on top of
    Android's own SELinux and seccomp sandbox. The internet permission is there for the mesh,
    which gets the TCP filter.

## Unsafe code

Safe Rust can't make system calls, cross the Java boundary, or manage memory page by page. Each place below does one of those. Every block is as small as it can be and carries a `SAFETY` comment. Everything else is safe Rust.

### hushphone

1. **`src/harden.rs:12`.** Before anything else runs, the process calls `umask(0o077)`, `prctl(PR_SET_DUMPABLE, 0)` and `setrlimit(RLIMIT_CORE, 0)`. As a result:
   - every file it creates is its owner's alone;
   - no core dump is written;
   - the user's other processes can't ptrace it or read `/proc/<pid>/mem`.

   These are plain libc calls with constant arguments, and the standard library has no safe wrapper for any of them.
2. **`src/android.rs:26`.** `#[unsafe(no_mangle)]` on `android_main`, so that android-activity finds the entry point by its symbol name. The attribute is unsafe because an exported name could clash with another symbol, and this one is unique to the app.
3. **`src/android.rs:72`.** `JavaVM::from_raw` wraps the pointer to the Java VM that android-activity hands over. JNI is a C interface, so wrapping its raw pointer can't be checked. The VM outlives the app.
4. **`src/android.rs:74`.** `JObject::from_raw` wraps the activity's global reference, also from android-activity. It's needed for the Keystore, permission requests, the document picker and binding to kpclientd-android, and it stays valid while the activity lives.
5. **`src/android.rs:162`, `:172` and `:182`.** `#[unsafe(no_mangle)]` on `Java_network_katzenpost_hushphone_FilePicker_nativeOnResult` and `Java_network_katzenpost_hushphone_QrScanner_nativeOnResult`, the methods the picker activity in `android/FilePicker.java` and the scanner in `android/QrScanner.java` call with the document the user chose or the QR code's text, and on `Java_network_katzenpost_hushphone_QrScanner_nativeDecode`, which the scanner calls with each camera frame. JNI finds them by these names. The first two only hand the string on to the thread waiting for it. At `:199`, `slice::from_raw_parts` lends the frame's direct buffer, at the address and length JNI reports for it, to the QR decoder for the length of the call; the frame's width, height and row stride are checked against that length first.
6. **`src/android.rs:276` and `:429`.** `OwnedFd::from_raw_fd` takes over a descriptor from Java: the document the user picked, opened by the content resolver, and the socket kpclientd-android hands back. `ParcelFileDescriptor.detachFd` gives up ownership on the Java side first, so each descriptor has exactly one owner, which closes it on drop.
7. **`tests/sandbox.rs:70`** (test only). `prctl(PR_GET_DUMPABLE)` and `getrlimit(RLIMIT_CORE)` check that item 1 took effect.

### pigeonhole_protocols: `Secret` (`rust/src/secret/mod.rs`)

All of these are inside `Secret`, the type that holds every key and capability (see below).

8. **Line 24–25.** `unsafe impl Send` and `Sync`. A raw pointer is neither by default. The guarded memory belongs to the `Secret` alone and is never written after it's made, so sharing or moving it between threads is sound.
9. **Line 31.** `memsec::malloc_sized`: allocates the guarded memory.
10. **Line 33.** `NonNull::as_mut`: copies the secret in. The memory is exactly `bytes.len()` bytes and no one else holds it.
11. **Line 52.** `NonNull::as_ref`: lends the bytes out as `&[u8]`. The borrow can't outlive the `Secret`, so it can't outlive the memory either.
12. **Line 69.** `memsec::free`: wipes the memory and unmaps it, once, in `Drop`. It aborts if the canary in front of the data was overwritten.
13. **Line 91.** `memsec::memeq`: compares two secrets of equal length in constant time.

### rustic_audio: `SecureVec` (`src/secure.rs`)

All of these are inside `SecureVec`, a growable vector in guarded memory. It holds voice samples and encoded clips.

14. **Line 22–23.** `unsafe impl Send` and `Sync`, for `T` that is itself `Send` or `Sync`. The memory belongs to the vector alone. Recording fills it on the audio thread, and it is then handed to the caller.
15. **Line 31.** `memsec::malloc_sized` for `cap` values. memsec ends the allocation on a page boundary and the byte count is a multiple of `T`'s size, so the start is aligned for `T`. A debug assertion checks this.
16. **Line 42** (Android only). `sysconf` and `madvise(MADV_DONTDUMP)`. memsec leaves pages out of core dumps on Linux but not on Android, so this does it there.
17. **Line 61.** `memsec::free`: wipes and unmaps, with the same pointer and size it was allocated with. It's called on drop, and when the vector grows into a new allocation.
18. **Line 109.** `ptr::copy_nonoverlapping`: moves the values into the larger allocation when the vector grows. The two allocations are distinct, and both hold `len` values.
19. **Line 117.** `slice::from_raw_parts`: reads out the `len` values when guarded memory is refused and the vector falls back to the heap.
20. **Line 139.** `ptr::copy_nonoverlapping`: appends after the `len` values, into room just made by `reserve`.
21. **Line 165 and 175.** `slice::from_raw_parts` and `from_raw_parts_mut`: lend out exactly the `len` initialised values, borrowed as the vector is.

### Why memsec needs unsafe

memsec is a Rust port of libsodium's `sodium_malloc`. Every function in its API is an `unsafe fn` that takes or returns raw pointers. Guard pages, `mlock` and `madvise` work on raw pages, so no safe interface can provide them. Crates that wrap guarded memory safely, such as `secrets`, do the same inside.

The wrappers in items 8–21 take on what the compiler can't check:
- each allocation is freed exactly once, with the pointer and size it came from;
- nothing is used after it's freed;
- every access stays within the allocated length.

Those are the lines to read hardest in an audit. A mistake there means memory corruption, not just a leak.

## Keeping critical data in memory

Two tools are used, for two different jobs.

### memsec: guarded memory for long-lived secrets

`Secret` (pigeonhole_protocols) and `SecureVec` (rustic_audio) allocate through memsec. For each allocation memsec:
- maps fresh pages for it alone;
- places an inaccessible guard page before and after it, so that overrunning a neighbouring buffer faults instead of reading the secret;
- puts a random canary in front of the data and checks it on free, aborting if it changed;
- `mlock`s it, so it isn't written to swap;
- marks it `MADV_DONTDUMP`, so it stays out of core dumps (on Android, `SecureVec` does this itself; see item 15);
- wipes it before unmapping.

If the system refuses guarded memory (for example, when the `mlock` limit is reached), both types fall back to the ordinary heap and still wipe on drop. `is_guarded()` reports which happened.

**What `Secret` holds:**
- the read and write capabilities of every group's streams, which let anyone holding them read or write the group;
- the keys inside a contact voucher, and the voucher itself;
- payloads on their way through the transport;
- the state key, which encrypts the state file and attachments at rest. hushphone holds it in a `Secret` from the moment it's read from the keyring or unwrapped by the Android Keystore (`src/keys.rs`, `src/android.rs`). A new key is generated into a `Zeroizing` buffer and moved into a `Secret`.

`Secret` also compares in constant time, and its `Debug` prints only the length.

**What `SecureVec` holds:**
- voice notes, recorded straight into memory rather than to a file. The microphone's samples go into a `SecureVec<f32>`, and the encoded Ogg Opus clip into a `SecureVec<u8>`;
- the encoder's pending packets. Its scratch frame and packet buffers are wiped when it finishes.

A recording therefore never reaches the disk unencrypted. It leaves guarded memory only when it's sealed and sent.

### zeroize: wiping short-lived copies

Data that passes through only briefly, or has to be an ordinary `Vec` for another API, is held in `zeroize::Zeroizing`. It is wiped when it's dropped, but has no guard pages and isn't locked. This covers:
- **The decrypted state.** The whole redb database is decrypted into a `Zeroizing<Vec<u8>>` (pigeonhole `client/sealed_file.rs`). When it shrinks, the cut-off tail is wiped. When it grows, it moves into a new `Zeroizing` allocation and the old one is wiped, so growth leaves no copy behind.
- **Decrypted attachments and voice clips.** These come back from the store as `Zeroizing<Vec<u8>>` (`Attachments::read` in `src/backend.rs`). They are read on the client's thread to learn their type and length, and again for Save and playback, never on the window's thread.
- **Files on their way out.** The file picked to send (`Picked` in `src/effects.rs`, or the document the picker gave on Android) and the copy of a recorded clip for the client (`Cmd::SendFile` in `src/backend.rs`) are `Zeroizing` until the client takes them.
- **The contacts.** The names given to members, with their keys, are read and written through `Attachments` (`src/contacts.rs`), so they are sealed on disk with the state.
- **Files being sealed.** When an existing plain state is sealed (`Store::reseal`), each file is read into `Zeroizing` before it's encrypted.
- **The Keystore round trip on Android.** `through` in `src/android.rs` wraps or unwraps the state key with the Keystore. It wipes the Java byte arrays on both sides with `Arrays.fill`, and returns its result in `Zeroizing`.
- **Heap fallbacks.** When `Secret` and `SecureVec` fall back to the heap, they wipe there too, and `From<Vec<u8>> for Secret` wipes the vector it consumes.

### Around the process

Wiping memory matters little if the whole process can be read. `harden::process` (item 1) prevents core dumps and ptrace or `/proc/<pid>/mem` access from the user's other processes. `harden::data_dir` makes the data directory owner-only. On Android, `FLAG_SECURE` keeps the app out of screenshots, screen recordings and the recent-apps thumbnails. A seccomp filter on every thread, and on Debian the AppArmor profile in `packaging/apparmor`, keep it from starting programs or tracing and reading other processes (see What hushphone does above). The seccomp filter is not optional: if it cannot be applied, the app does not connect to the daemon. The AppArmor profile also keeps the app off everything on the session bus but the keyring, since the user's systemd and the portals would otherwise start programs and open files for it, and off every unix socket but the daemon's, the display's and the sound server's.

## Known gaps

These are the places where critical data can still be left in ordinary memory, unwiped:
- **Playback.** `Voice::play` moves the clip out of its `Zeroizing` buffer into the plain `Vec` that rustic_audio's `play_received_bytes` takes. The decoder's PCM output is not wiped either.
- **The database's working copies.** redb reads pages out of the decrypted buffer into its own `Vec`s and caches them. Those aren't wiped, and neither are the records decoded from them.
- **The user interface.** Message text, group names and member names become ordinary `String`s and Slint values in order to be shown.
- **The keyring and the Keystore.** On the desktop, the secret the keyring returns (oo7) and the D-Bus messages that carry it are outside our control. On Android, the JVM may copy byte arrays before `Arrays.fill` reaches them.
- **The client's copy of what it sends.** `Client::send_file` takes a plain `Vec<u8>` and keeps it until it is sealed and written.
- **Files the user saves.** Save writes the file plain into `~/hushphone/received`, or into the document the system's picker gave on Android. That is what saving is for.
- **Heap fallback.** When guarded memory is refused, secrets are still wiped, but can be swapped out or end up in a dump.

Once the process exits, the operating system reclaims its memory, so these gaps matter only for an attacker who can read the memory of a running hushphone. That's what the hardening above is there to prevent.

## Secure interaction design

Memory protection does nothing for a user who is misled into the wrong action. Ka-Ping Yee's chapter [1] asks how a system can protect the interests of its legitimate user, and offers ten guidelines and two strategies for it. Yee's core point is that security comes from what the user initiates, not from warnings the user has to react to. What hushphone does, above, lists its measures, following Paw's GUI security guidelines. This section relates those measures to Yee's terms.

### The two strategies

- **Security by designation.** The act of choosing something also grants the authority to do it, so there's no separate permission step. `UserIntent` (`src/bridge.rs:24`) is this rule in the type system. Only the UI's callbacks can make one, and copying, pasting, picking, saving a file and starting the microphone each require one. Nothing that arrives from the network can trigger them. Files follow Yee's "Securing file access": the desktop's file chooser is not used, because it returns a name and leaves the app to open it with the user's whole authority. On Linux the user puts a file in `~/hushphone/send` and picks it in the app, and the sandbox lets the app read nothing else of theirs; on Android the system's picker hands over a descriptor for the one document chosen: the receiver, not the telephone.
- **User-assigned identifiers.** Names chosen by other parties are a way to attack the user, so the user can assign their own. A member's petname (`src/settings.rs`, `src/model.rs`) is kept locally, shown in bold, and tied to the member's key. A name the member chose for themselves is always marked `~name`, so it can't pass for one of the user's own.

### The guidelines

Each number below is Yee's. Only the guidelines hushphone has a clear answer to are listed.

- **1. Least authority on the easiest path.** The sandbox applies without any setup. On Linux, hushphone gets the daemon's socket, the display and the sound server, no IP, the keyring on the session bus and nothing else there, and of the user's files only `~/hushphone/send` to read and `~/hushphone/received` to write. On Android, it doesn't ask for the storage permissions; on the mixnet kpclientd-android hands it connected sockets, and the internet permission is used only for the mesh's one TCP link to a replica; and the document picker hands it one file at a time.
- **2. Authority follows consent.** See `UserIntent` above. Received files are never saved on their own, and joining a group takes a voucher code that the user passes on, and that a member adds by hand.
- **3. Authority can be taken back.** In kpclientd-android, **Forget** withdraws an app's access and ends its connections at once.
- **4. Awareness of others' authority.** The recording indicator shows for exactly as long as the microphone is open, and kpclientd-android's **Apps** list shows every app the user has answered about.
- **6. A protected channel to the agent.** The app's own notices use a frame that no message can draw, and message content is clipped to its bubble.
- **9. Distinguishable, truthful appearances.** Names lose every default-ignorable character, those Unicode renders as nothing, and runs of whitespace (`src/text.rs`); names that look alike show 64 bits of the member's key, beyond a member's reach to match; a file's type is read from its bytes, not its name; and a saved file's name loses path separators and a leading dot.
- **10. Clear consequences.** A dangerous button in a confirmation waits 0.7 s and the safe one has the focus. Every control ignores clicks until it has been still, and about the same message, for half a second, so an arriving message can't move a different control under the user's finger, and a row that comes to show another message can't take a click meant for the last one.

### Where we still rely on admonition

Yee treats warnings as the fallback for when designation isn't possible, and warns that forced prompts train users to dismiss them. Two remain:
- **The no-keyring warning.** It's a strip under the group list that doesn't interrupt the user, which is the form Yee recommends.
- **kpclientd-android's approval question.** This one is a prompt. The daemon can't tell by itself which apps the user trusts, so it has to ask. It's asked once per app and signing key, and **Allow** only takes a tap after a second.

## References

1. Ka-Ping Yee, "Guidelines and Strategies for Secure Interaction Design", chapter 13 of Lorrie Faith Cranor and Simson Garfinkel (eds.), *Security and Usability: Designing Secure Systems That People Can Use*, O'Reilly, 2005, pp. 253–279. <http://sid.toolness.org/ch13yee.pdf>
