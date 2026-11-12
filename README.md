# hushphone

Group chat with push-to-talk voice messages, for Linux and Android. It runs over the
[Katzenpost](https://github.com/katzenpost) mixnet or a [Reticulum](https://reticulum.network) mesh.
Mesh and mixnet members share groups through a bridge replica, and the groups are the same ones
katzenqt, jam, frank_chat and katzenrns use.

<p>
  <img src="docs/desktop.png" alt="hushphone on the Linux desktop: a group with a voice clip, messages from a jam user and read receipts" width="640">
  <img src="docs/android.png" alt="hushphone on Android, in the same group" width="210">
</p>

hushphone is a client. It does no mixnet networking itself: on the mixnet it needs a running
`kpclientd`, and on the mesh it needs a Pigeonhole replica's address. There are no accounts:
members join a group with a voucher, shown as a QR code or sent by sound.

## Quick start

Linux, on the mesh, through namenlos's bridge replica. You need [Rust](https://rustup.rs).

```sh
sudo apt install git pkg-config libasound2-dev libfontconfig-dev
git clone https://github.com/katzenpost/hushphone && cd hushphone
cargo run --release -- --transport reticulum --replica 162.220.11.225:4242
```

On the mixnet instead, start `kpclientd` and run `cargo run --release`; see
[docs/building.md](docs/building.md). On Android, install the APK from the
[releases](https://github.com/katzenpost/hushphone/releases), and for the mixnet
[kpclientd-android](https://github.com/katzenpost/kpclientd-android) too; see
[docs/android.md](docs/android.md).

## Documentation

- [Building and running](docs/building.md): the three transports, the Debian package, releases
- [Android](docs/android.md) and [flags](docs/flags.md)
- [Using it](docs/using.md), with radio mode and ping, and [vouchers](docs/vouchers.md)
- [Security](docs/SECURITY.md): what protects the state, memory and process, and the known gaps

The code is in `src/` (`backend.rs` the client, `bridge.rs` and `model.rs` between it and the
window, `mesh.rs` the Reticulum transports, `voice.rs` push to talk, `sound.rs` vouchers by
sound), the window in `ui/` (Slint), and the Android glue in `android/` and `packaging/android/`.

## Status and compatibility

This tree is version 0.2.1. hushphone writes katzenqt1 wire version 4 by default, or 3 with
`--wire-version 3`, and reads both. Voice clips are Ogg Opus, sent as files as frank_chat and
katzenqt send them. Vouchers by sound are checked against ggwave vectors in `tests/data/ggwave`,
taken from ggwave-rs and pigeonhole_reticulous.

## Testing and licence

```sh
make check test
```

The tests cover the model, two clients joining and talking over an in-memory Pigeonhole, the
window and the seccomp sandbox. `tests/namenlos_interop.rs` runs against a live `kpclientd` and
only when `HUSH_INTEROP` is set. AGPL-3.0-only; see [LICENSE](LICENSE).
