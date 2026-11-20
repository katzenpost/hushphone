//! cpal 0.17.3 as it is, except for `default_host()` on OpenBSD, where cpal has no host, and on
//! FreeBSD (and so HardenedBSD), where its ALSA host, through alsa-plugins' OSS plugin, drops
//! about a tenth of what it records: there it is a host of our own over sndio or OSS.
//! rustic_audio and hushphone both play and record through `cpal::default_host()`, so
//! hushphone's Cargo.toml patches this crate in for crates.io's cpal.

pub use upstream::*;

#[cfg(any(target_os = "openbsd", target_os = "freebsd"))]
mod host;

#[cfg(target_os = "openbsd")]
#[path = "sndio.rs"]
mod io;

#[cfg(target_os = "freebsd")]
#[path = "oss.rs"]
mod io;

#[cfg(any(target_os = "openbsd", target_os = "freebsd"))]
pub fn default_host() -> Host {
    Host::from(platform::CustomHost::from_host(host::Host))
}
