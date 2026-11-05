# hushphone

Group chat with push-to-talk voice messages, for Linux and Android. It runs over the
[Katzenpost](https://katzenpost.network) mixnet or over a [Reticulum](https://reticulum.network)
mesh, and members on the two share groups through a bridge replica. It is written in Rust with a
[Slint](https://slint.dev) interface.

hushphone speaks katzenqt's group chat protocol on [Pigeonhole](https://github.com/katzenpost/pigeonhole)
boxes, so its groups are shared with katzenqt, [jam](https://github.com/katzenpost/jam),
[frank_chat](https://github.com/katzenpost/frank_chat) and, on the mesh,
[katzenrns](https://github.com/katzenpost/katzenrns). Voice messages are Ogg Opus clips recorded
with [rustic_audio](https://github.com/katzenpost/rustic_audio), sent as frank_chat and katzenqt
send them: a file of type `audio/opus` named by its SHA-256.

<p>
  <img src="docs/desktop.png" alt="hushphone on the Linux desktop: a group with a voice clip, messages from a jam user and read receipts" width="640">
  <img src="docs/android.png" alt="hushphone on Android, in the same group" width="210">
</p>

The same group on the desktop and on Android, over the namenlos mixnet. jules is using jam.

## Quick start

### Linux, on the mesh

The quickest way in: no daemon, just one TCP link to namenlos's bridge replica, which is on both
the mesh and the mixnet. You need [Rust](https://rustup.rs) (rustup fetches the pinned version by
itself) and:

```sh
sudo apt install git pkg-config libasound2-dev libfontconfig-dev
git clone https://github.com/katzenpost/hushphone && cd hushphone
cargo run --release -- --transport reticulum --replica 162.220.11.225:4242
```

The status bar reads "On the Reticulum mesh" within about twenty seconds. Then start a group, or
join one (see [Using it](#using-it)).

### Linux, on the mixnet

hushphone does no mixnet networking of its own: the Katzenpost client daemon, `kpclientd`, does
it. Build it ([Go](https://go.dev/dl/) 1.26 or later) and start it with the namenlos config:

```sh
git clone --depth 1 https://github.com/katzenpost/katzenpost && cd katzenpost
make kpclientd
cmd/kpclientd/kpclientd -c docker/client-configs/namenlos.toml
```

It listens on the abstract socket `@katzenpost`, where hushphone looks by default. Then, in the
hushphone checkout:

```sh
cargo run --release
```

The status bar reads "kpclientd is on the mixnet" once the daemon has reached a gateway.

### Android

Install two APKs from their latest GitHub releases, `arm64-v8a` for a phone (`x86_64` for an
emulator):

- [kpclientd-android](https://github.com/katzenpost/kpclientd-android/releases/latest), which runs
  kpclientd on the phone;
- [hushphone](https://github.com/katzenpost/hushphone/releases/latest).

Start hushphone and allow it when kpclientd-android asks. On Android, hushphone uses the mixnet.

## Transports

`--transport` picks how hushphone reaches Pigeonhole. The groups, the protocol and the window are
the same on all three.

| | `mixnet` (default) | `reticulum` | `reticulum-py` |
|---|---|---|---|
| Network | the Katzenpost mixnet | a Reticulum mesh | a Reticulum mesh |
| Through | kpclientd, over its thin client socket | [Reticulum-rs](https://github.com/BeechatNetworkSystemsLtd/Reticulum-rs), in hushphone's own process | the official Python [RNS](https://github.com/markqvist/Reticulum), in hushphone's own process |
| Build | default | default | `--features reticulum-py`, and `python3-dev` |
| Needs at run time | a running kpclientd | a replica's address | a replica's address or a Reticulum config, and the Python packages below |
| Platforms | Linux, Android | Linux | Linux |

**mixnet** hides who talks to whom from the network: kpclientd sends hushphone's messages
through the Katzenpost mix nodes to the replicas.

```sh
cargo run --release -- --transport mixnet --daemon @katzenpost
```

**reticulum** is pure Rust. hushphone starts a Reticulum instance of its own with one TCP link to
a replica's Reticulum interface. It needs nothing else
installed, so it is the mesh transport to pick unless you want the Python one.

```sh
cargo run --release -- --transport reticulum --replica 162.220.11.225:4242
```

**reticulum-py** reaches the same mesh through the reference Reticulum implementation. It is not
a Python program: hushphone stays one Rust binary, which embeds CPython through
[PyO3](https://pyo3.rs) (that is, CPython's C API) and calls the Python packages `rns`,
`pigeonhole_rns`, `pigeonhole_protocols` and `hpqc` in-process. Pick it to run against the same
RNS as the rest of your Reticulum programs, or to use an existing Reticulum config, with all its
interfaces, through `--rns-config DIR`. The packages are looked for in the virtual environment
`VIRTUAL_ENV` names, or system-wide; compact vouchers need hpqc's `katzenqt1-v4` branch:

```sh
sudo apt install python3-dev python3-venv
python3 -m venv ~/.venvs/hushphone
~/.venvs/hushphone/bin/pip install rns \
    "hpqc @ git+https://github.com/katzenpost/hpqc@katzenqt1-v4#subdirectory=py" \
    "pigeonhole_protocols @ git+https://github.com/katzenpost/pigeonhole_protocols#subdirectory=py" \
    "pigeonhole_rns @ git+https://github.com/katzenpost/pigeonhole_rns"
VIRTUAL_ENV=~/.venvs/hushphone cargo run --release --features reticulum-py -- \
    --transport reticulum-py --replica 162.220.11.225:4242
```

**Across the bridge.** Members on the mesh and on the mixnet share groups through a bridge
replica, such as namenlos's at `162.220.11.225:4242`. Either side can found a group and add the
other with a voucher, with one exception for now: a group founded on the mixnet can add a mesh
member only from a long voucher, so a mesh user joining one starts hushphone with
`--wire-version 3` until kpclientd reads the compact form.

`--replica` writes a Reticulum config for that one link under the data directory
(`reticulum/config`), on every start; the identity beside it stays. That instance is hushphone's
own: it does not touch `~/.reticulum` and is not shared with other Reticulum programs.

## Flags

| Flag | |
|---|---|
| `--transport mixnet\|reticulum\|reticulum-py` | how Pigeonhole is reached (default `mixnet`) |
| `--daemon ADDRESS` | mixnet: kpclientd's thin client socket: `@name`, a path, or `host:port` (default `@katzenpost`) |
| `--replica HOST:PORT` | reticulum, reticulum-py: a replica's Reticulum TCP interface, over one TCP link from a Reticulum instance of hushphone's own |
| `--rns-config DIR` | reticulum-py: the Reticulum instance `DIR` describes, instead of `--replica` |
| `--box-size N` | reticulum, reticulum-py: the box plaintext size (default namenlos's, 1549) |
| `--sim SOCKET` | for testing: `pigeonhole-sim` instead of a transport |
| `--memory` | for testing: an in-memory Pigeonhole; nothing leaves the process |
| `--data DIR` | where state, settings and received files live (default `$XDG_DATA_HOME/hushphone`) |
| `--files DIR` | the one directory hushphone reads your files from and writes them to (default `~/hushphone`) |
| `--name NAME` | your name, on the first start; otherwise hushphone asks |
| `--wire-version 3\|4` | the katzenqt1 wire version written and minted: 3 (map form, long vouchers) or 4 (compact; default); saved for later starts; both are read |

Flags that do not go with the transport, such as `--daemon` with `reticulum`, are refused.

## Debian package

`make deb` builds `dist/hushphone_<version>_amd64.deb` in a Debian 13 build container, with
podman. The package installs the app, its desktop entry and its icon:

```sh
make deb                      # or: make deb DISTRO=ubuntu-24.04
sudo apt install ./dist/hushphone_*.deb
```

Its AppArmor profile allows no IP sockets, so the Reticulum transports and a kpclientd at
`host:port` need the TCP lines the profile's head comment gives, in
`/etc/apparmor.d/local/hushphone`. The package is built without `reticulum-py`.

The package is reproducible. `make deb-ci`, run in the build container as CI does, builds it,
installs and tests it, then builds it again in another directory and checks that both packages
are the same.

## Android

hushphone reaches the mixnet through
[kpclientd-android](https://github.com/katzenpost/kpclientd-android), which runs kpclientd on the
phone and lets in only the apps you allow.

- **Asking.** On its first start, hushphone asks kpclientd-android for a connection.
  kpclientd-android then asks you whether hushphone may use the mixnet.
- **Connecting.** Once you allow it, every connection hushphone makes comes from kpclientd-android
  as a unix socket. hushphone joins it to a socket in its own private directory, which its thin
  client dials.
- **No network.** hushphone has no internet permission and opens no IP socket.

To build the APK:

```sh
cargo install cargo-ndk --locked --version 4.1.2
export ANDROID_HOME=~/Android/Sdk
make apk ABI=arm64-v8a      # or ABI=x86_64 for an emulator; dist/hushphone-<abi>.apk, debug-signed
```

The APK is put together by `scripts/build-apk.sh` from the library cargo-ndk builds, the
manifest in `packaging/android` and the one Java class in `android/`, which opens the system's
document picker for the app (see Files below).

hushphone sets `FLAG_SECURE`, so its window is kept out of screenshots, screen recordings and the
recent-apps view. Test builds made with `--features test-screenshots` leave that flag off; never
ship one.

## Using it

- **Start a group** with New group. Only you see the name you give it.
- **Join someone's group** with Join a group. hushphone mints a voucher, over the mixnet or the mesh, and
  shows its code. Give the code to a member of the group over a channel you trust; when they add it with
  Add a member, you join. [docs/vouchers.md](docs/vouchers.md) walks through both sides with
  screenshots.
- **Talk** by holding the microphone button, or Space while it has focus; let go to send. Escape,
  or dragging away, cancels.
- **Files.** On Linux, hushphone touches one directory of yours, `~/hushphone`. To send a file,
  put it in `~/hushphone/send` first, then press the paperclip and pick it there. Save puts a
  received file in `~/hushphone/received`, never over another. On Android, the paperclip and
  Save open the system's document picker, which hands hushphone that one file.
- **Marks** on your messages: a clock while queued, ✓ once written to the replicas, ✓✓ once a
  member has acknowledged reading them.
- **F10** hides the window's contents at once.
- **Theme** follows the system, and is dark when the system does not say. Settings can fix it to light or dark.

## Security

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
- **On disk.** Settings (your name, the daemon's address, the theme, the wire version) are written with mode 0600
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
  - **Android.** hushphone gets its connections to kpclientd from kpclientd-android, as unix
    sockets, so the unix-only filter applies there too, on top of Android's own SELinux and
    seccomp sandbox. The app does not even ask for the internet permission.
- **In memory.** Where the code leaves safe Rust, and how keys, caps and voice clips are kept in
  guarded memory and wiped, is set out in [docs/SECURITY.md](docs/SECURITY.md).

## Tests

```sh
SLINT_EMIT_DEBUG_INFO=1 cargo test
```

The tests cover the following:

- The model, text handling, voice naming, settings, the sealed contacts, and the files
  directory: what is listed, what can be picked, and that saving never overwrites.
- Two clients over an in-memory Pigeonhole: they join by voucher, exchange text and a voice clip,
  and acknowledge each other.
- The window on mocked time: controls ignore early clicks, clicks after moving and clicks after
  their row came to show another message; a confirmation waits; push-to-talk works; the attach
  dialog sends what was picked.
- A whole session driven through the window, a file sent from the send directory included.

## Releases

A `v*` tag runs the tests, then builds the Linux binary and the Android APKs (`arm64-v8a` for
phones, `x86_64` for emulators) twice each, on different machines and in different directories,
and the Debian packages for Debian 13 and Ubuntu 24.04 twice each. The release is published only
if both builds of each are byte for byte the same. Release builds are reproducible:

- `rust-toolchain.toml` pins Rust.
- The Linux build runs in an Ubuntu 22.04 image pinned by digest, with packages from one Ubuntu
  snapshot.
- The Android build pins the NDK, build tools, platform, JDK and cargo-ndk, adds the APK's
  entries in a fixed order with their times set, and makes one APK per ABI.
- Paths and dates are taken out of the output.

Each step is a make target, which CI calls as it is:

| Target | |
|---|---|
| `make check test` | clippy and the tests |
| `make release-linux` | the Linux tarball, into `dist/` |
| `make android-tools release-android` | the pinned SDK pieces and cargo-ndk, then the APKs, into `dist/` |
| `make deb-ci-image DISTRO=…` | the Debian package, built twice in its build image and tested |

To check a release against its source:

```sh
git checkout v0.1.3
img=ubuntu:22.04@sha256:8a2321a92b438238f93b4346fff61bc4259c99fdd4d9b52ee86f153fee0c344f
podman run --rm -e VERSION=v0.1.3 -v $PWD:/src -w /src $img \
    sh -c 'scripts/linux-env.sh && . /root/.cargo/env && make release-linux'
sha256sum dist/*.tar.gz
```

The APKs are signed, so a rebuild without the key differs only in its signature. Compare their
entries with `unzip -v`. To rebuild them, use JDK 21 and run
`VERSION=v0.1.3 make android-tools release-android`, with `HUSHPHONE_KEYSTORE` and
`HUSHPHONE_KEYSTORE_PASSWORD` naming a keystore of your own.

## License

AGPL-3.0-only; see [LICENSE](LICENSE).
