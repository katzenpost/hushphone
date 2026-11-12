# Building and running

## Linux, on the mesh

The quickest way in: no daemon, just one TCP link to namenlos's bridge replica, which is on both
the mesh and the mixnet. You need [Rust](https://rustup.rs) (rustup fetches the pinned version by
itself) and:

```sh
sudo apt install git pkg-config libasound2-dev libfontconfig-dev
git clone https://github.com/katzenpost/hushphone && cd hushphone
cargo run --release -- --transport reticulum --replica 162.220.11.225:4242
```

The status bar reads "On the Reticulum mesh" within about twenty seconds. Then start a group, or
join one (see [using.md](using.md)).

## Linux, on the mixnet

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

## Transports

`--transport` picks how hushphone reaches Pigeonhole. The groups, the protocol and the window are
the same on all three.

| | `mixnet` (default) | `reticulum` | `reticulum-py` |
|---|---|---|---|
| Network | the Katzenpost mixnet | a Reticulum mesh | a Reticulum mesh |
| Through | kpclientd, over its thin client socket | [berlinmesh/reticulum](https://git.emcomm.cc/berlinmesh/reticulum), in hushphone's own process | the official Python [RNS](https://github.com/markqvist/Reticulum), in hushphone's own process |
| Build | default | default | `--features reticulum-py`, and `python3-dev` |
| Needs at run time | a running kpclientd | a replica's address | a replica's address or a Reticulum config, and the Python packages below |
| Platforms | Linux, Android | Linux, Android | Linux |

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
`VIRTUAL_ENV` names, or system-wide; pinned to the commits hushphone builds on:

```sh
sudo apt install python3-dev python3-venv
python3 -m venv ~/.venvs/hushphone
~/.venvs/hushphone/bin/pip install rns \
    "hpqc @ git+https://github.com/katzenpost/hpqc@4732d7fb7e6328407f69a5cea0dfb5e9eca308f7#subdirectory=py" \
    "pigeonhole_protocols @ git+https://github.com/katzenpost/pigeonhole_protocols@3264448de0ec01ecd0adc70bf4a6150e8e58ed6e#subdirectory=py" \
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
