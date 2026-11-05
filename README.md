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

What hushphone guards against, and how, is set out in [docs/SECURITY.md](docs/SECURITY.md).

## Tests

```sh
SLINT_EMIT_DEBUG_INFO=1 cargo test
```

The tests cover the model, two clients joining and talking over an in-memory Pigeonhole, and
the window on mocked time.

## Releases

A `v*` tag builds the Linux tarball, the Android APKs and the Debian packages twice each, and
publishes them only if both builds are byte for byte the same. The steps are make targets
(`make check test`, `make release-linux`, `make android-tools release-android`,
`make deb-ci-image`); see the `Makefile` and `.github/workflows`.

## License

AGPL-3.0-only; see [LICENSE](LICENSE).
