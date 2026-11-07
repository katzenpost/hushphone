use std::path::PathBuf;

use hushphone::Setup;
use hushphone::backend::Source;

const USAGE: &str = "usage: hushphone [--transport mixnet|reticulum|reticulum-py] [--daemon ADDRESS]
                 [--replica HOST:PORT | --rns-config DIR] [--box-size N] [--sim SOCKET | --memory]
                 [--data DIR] [--files DIR] [--name NAME] [--wire-version 3|4] [--camera DEVICE]

  --transport T     how Pigeonhole is reached: mixnet, the Katzenpost mixnet through kpclientd
                    (default); reticulum, a Reticulum mesh in pure Rust; or reticulum-py,
                    the mesh through the Python RNS, embedded (built with --features reticulum-py)
  --daemon ADDRESS  mixnet: kpclientd's thin client socket: @name, a path, or host:port (default @katzenpost)
  --replica HOST:PORT  reticulum, reticulum-py: a replica's Reticulum TCP interface, reached over one
                    TCP link from a Reticulum instance of hushphone's own
  --rns-config DIR  reticulum-py: the Reticulum instance DIR describes, instead of --replica
  --box-size N      reticulum, reticulum-py: the box plaintext size (default namenlos's, 1549)
  --sim SOCKET      for testing: pigeonhole-sim at SOCKET instead of a transport
  --memory          for testing: an in-memory Pigeonhole; nothing leaves this process
  --data DIR        keep state in DIR (default $XDG_DATA_HOME/hushphone)
  --files DIR       send files from DIR/send and save received ones in DIR/received (default ~/hushphone)
  --name NAME       your name, on the first start
  --wire-version N  write and mint katzenqt1 wire version 3 (map form, long vouchers) or 4 (compact; default)
  --camera DEVICE   the webcam that scans voucher QR codes (default the first, /dev/video0…)";

fn main() {
    #[cfg(target_os = "linux")]
    hushphone::harden::process();
    let mut daemon = None;
    let mut source = None;
    let mut data = None;
    let mut files = None;
    let mut camera = None;
    let mut name = None;
    let mut wire_version = None;
    let mut replica: Option<String> = None;
    let mut rns_config: Option<PathBuf> = None;
    let mut transport = "mixnet".to_string();
    let mut box_size = hushphone::mesh::BOX_SIZE;
    let mut seen = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || {
            args.next().unwrap_or_else(|| {
                eprintln!("{a} needs a value\n{USAGE}");
                std::process::exit(2)
            })
        };
        seen.push(a.clone());
        match a.as_str() {
            "--transport" => transport = value(),
            "--daemon" => daemon = Some(value()),
            "--sim" => source = Some(Source::SimSocket(value().into())),
            "--memory" => source = Some(Source::Memory(Default::default())),
            "--data" => data = Some(PathBuf::from(value())),
            "--files" => files = Some(PathBuf::from(value())),
            "--camera" => camera = Some(PathBuf::from(value())),
            "--name" => name = Some(value()),
            "--wire-version" => match value().as_str() {
                "3" => wire_version = Some(3),
                "4" => wire_version = Some(4),
                v => {
                    eprintln!("--wire-version is 3 or 4, not {v}\n{USAGE}");
                    std::process::exit(2);
                }
            },
            "--replica" => replica = Some(value()),
            "--rns-config" => rns_config = Some(PathBuf::from(value())),
            "--box-size" => {
                let v = value();
                box_size = v.parse().unwrap_or_else(|_| {
                    eprintln!("--box-size is a number of bytes, not {v}\n{USAGE}");
                    std::process::exit(2)
                })
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ => {
                eprintln!("unknown argument {a}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let data_dir = data.clone().unwrap_or_else(|| dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("hushphone"));
    let cache_dir = match data {
        Some(d) => d.join("cache"),
        None => dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join("hushphone"),
    };
    if let Err(e) = check(&transport, &seen) {
        eprintln!("{e}\n{USAGE}");
        std::process::exit(2)
    }
    let mesh = match transport.as_str() {
        "reticulum" => {
            let r = replica.expect("checked");
            hushphone::mesh::host_port(&r).map(|_| Some(Source::Mesh(r.clone(), box_size)))
        }
        "reticulum-py" => python_source(replica, rns_config, box_size, &data_dir).map(Some),
        _ => Ok(None),
    };
    match mesh {
        Ok(Some(s)) => source = Some(s),
        Ok(None) => {}
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            std::process::exit(2)
        }
    }
    let files_dir = files.unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("hushphone"));
    let default_daemon = "@katzenpost".to_string();
    let setup = Setup {
        data_dir,
        cache_dir,
        files_dir,
        source: source.unwrap_or_else(|| Source::Daemon(daemon.clone().unwrap_or_default())),
        default_daemon: daemon.unwrap_or(default_daemon),
        name,
        wire_version,
        old_defaults: Vec::new(),
        keys: hushphone::backend::Keys::System,
        camera,
    };
    if let Err(e) = hushphone::run(setup) {
        eprintln!("hushphone: {e}");
        std::process::exit(1);
    }
}

/// Refuses flags that do not go with `transport` or with each other, `seen` being every flag given.
fn check(transport: &str, seen: &[String]) -> Result<(), String> {
    let has = |f: &str| seen.iter().any(|s| s == f);
    let refused: &[&str] = match transport {
        "mixnet" => &["--replica", "--rns-config", "--box-size"],
        "reticulum" => &["--daemon", "--rns-config"],
        "reticulum-py" => &["--daemon"],
        t => return Err(format!("--transport is mixnet, reticulum or reticulum-py, not {t}")),
    };
    if let Some(f) = refused.iter().find(|f| has(f)) {
        return Err(format!("{f} does not go with --transport {transport}"));
    }
    if (has("--sim") || has("--memory")) && has("--transport") {
        return Err("--sim and --memory take the place of a transport; give no --transport".into());
    }
    if has("--sim") && has("--memory") {
        return Err("--sim and --memory are two test sources; give one".into());
    }
    if has("--replica") && has("--rns-config") {
        return Err("--replica and --rns-config are two ways to reach the mesh; give one".into());
    }
    if transport == "reticulum" && !has("--replica") {
        return Err("--transport reticulum needs --replica HOST:PORT".into());
    }
    if transport == "reticulum-py" && !has("--replica") && !has("--rns-config") {
        return Err("--transport reticulum-py needs --replica HOST:PORT or --rns-config DIR".into());
    }
    Ok(())
}

#[cfg(feature = "reticulum-py")]
fn python_source(replica: Option<String>, rns_config: Option<PathBuf>, box_size: usize, data_dir: &std::path::Path) -> Result<Source, String> {
    let dir = match (replica, rns_config) {
        (Some(r), _) => hushphone::mesh::replica_config(data_dir, &r)?,
        (None, d) => d.expect("one of the two"),
    };
    Ok(Source::MeshPy(dir, box_size))
}

#[cfg(not(feature = "reticulum-py"))]
fn python_source(_: Option<String>, _: Option<PathBuf>, _: usize, _: &std::path::Path) -> Result<Source, String> {
    Err("this hushphone was built without the Python Reticulum; build it with --features reticulum-py, or use --transport reticulum".into())
}

#[cfg(test)]
mod tests {
    use super::check;

    fn ok(t: &str, flags: &[&str]) -> bool {
        check(t, &flags.iter().map(|f| f.to_string()).collect::<Vec<_>>()).is_ok()
    }

    #[test]
    fn refuses_flags_that_do_not_go_together() {
        assert!(ok("mixnet", &[]) && ok("mixnet", &["--daemon"]) && ok("mixnet", &["--sim"]));
        assert!(ok("reticulum", &["--transport", "--replica", "--box-size"]));
        assert!(ok("reticulum-py", &["--transport", "--rns-config"]) && ok("reticulum-py", &["--transport", "--replica"]));
        assert!(!ok("mixnet", &["--replica"]) && !ok("mixnet", &["--box-size"]));
        assert!(!ok("mixnet", &["--transport", "--memory"]) && !ok("mixnet", &["--sim", "--memory"]));
        assert!(!ok("reticulum", &["--transport"]) && !ok("reticulum", &["--transport", "--replica", "--daemon"]));
        assert!(!ok("reticulum", &["--transport", "--rns-config"]));
        assert!(!ok("reticulum-py", &["--transport"]) && !ok("reticulum-py", &["--transport", "--replica", "--rns-config"]));
        assert!(!ok("reticulum", &["--transport", "--replica", "--sim"]) && !ok("tor", &["--transport"]));
    }
}
