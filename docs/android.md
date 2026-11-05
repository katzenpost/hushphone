# Android

Install the `arm64-v8a` APKs (`x86_64` for an emulator) of
[kpclientd-android](https://github.com/katzenpost/kpclientd-android/releases/latest) and
[hushphone](https://github.com/katzenpost/hushphone/releases/latest). Start hushphone and allow it
when kpclientd-android asks. On Android, hushphone uses the mixnet only.

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
