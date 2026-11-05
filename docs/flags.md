# Flags

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
| `--camera DEVICE` | the webcam that scans voucher QR codes (default the first, `/dev/video0`…) |
| `--wire-version 3\|4` | the katzenqt1 wire version written and minted: 3 (map form, long vouchers) or 4 (compact; default); saved for later starts; both are read |

Flags that do not go with the transport, such as `--daemon` with `reticulum`, are refused.
