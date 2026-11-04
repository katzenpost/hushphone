use std::path::PathBuf;

use hushphone::Setup;
use hushphone::backend::Source;

const USAGE: &str = "usage: hushphone [--daemon ADDRESS] [--sim SOCKET] [--memory] [--data DIR] [--files DIR] [--name NAME] [--wire-version 3|4]
                 [--replica HOST:PORT | --rns-config DIR] [--box-size N]

  --daemon ADDRESS  kpclientd's thin client socket: @name, a path, or host:port (default @katzenpost)
  --sim SOCKET      use pigeonhole-sim at SOCKET instead of the mixnet
  --memory          use an in-memory Pigeonhole: nothing leaves this process
  --data DIR        keep state in DIR (default $XDG_DATA_HOME/hushphone)
  --files DIR       send files from DIR/send and save received ones in DIR/received (default ~/hushphone)
  --name NAME       your name, on the first start
  --wire-version N  write and mint katzenqt1 wire version 3 (map form, long vouchers) or 4 (compact; default)
  --replica HOST:PORT  reach a Pigeonhole replica over Reticulum, one TCP link from an instance of
                       hushphone's own (built with --features mesh)
  --rns-config DIR     use the Reticulum instance DIR describes instead (built with --features mesh)
  --box-size N         the mesh's box plaintext size (default namenlos's, 1549)";

fn main() {
    #[cfg(target_os = "linux")]
    hushphone::harden::process();
    let mut daemon = None;
    let mut source = None;
    let mut data = None;
    let mut files = None;
    let mut name = None;
    let mut wire_version = None;
    let mut replica: Option<String> = None;
    let mut rns_config: Option<PathBuf> = None;
    let mut box_size = 1549usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || {
            args.next().unwrap_or_else(|| {
                eprintln!("{a} needs a value\n{USAGE}");
                std::process::exit(2)
            })
        };
        match a.as_str() {
            "--daemon" => daemon = Some(value()),
            "--sim" => source = Some(Source::SimSocket(value().into())),
            "--memory" => source = Some(Source::Memory(Default::default())),
            "--data" => data = Some(PathBuf::from(value())),
            "--files" => files = Some(PathBuf::from(value())),
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
    if replica.is_some() || rns_config.is_some() {
        source = Some(mesh_source(replica, rns_config, box_size, &data_dir));
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
    };
    if let Err(e) = hushphone::run(setup) {
        eprintln!("hushphone: {e}");
        std::process::exit(1);
    }
}

#[cfg(feature = "mesh")]
fn mesh_source(replica: Option<String>, rns_config: Option<PathBuf>, box_size: usize, data_dir: &std::path::Path) -> Source {
    let dir = match (replica, rns_config) {
        (Some(_), Some(_)) => {
            eprintln!("--replica and --rns-config are two ways to reach the mesh; give one\n{USAGE}");
            std::process::exit(2)
        }
        (Some(r), None) => match hushphone::mesh::replica_config(data_dir, &r) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{e}\n{USAGE}");
                std::process::exit(2)
            }
        },
        (None, d) => d.expect("one of the two"),
    };
    Source::Mesh(Some(dir), box_size)
}

#[cfg(not(feature = "mesh"))]
fn mesh_source(_: Option<String>, _: Option<PathBuf>, _: usize, _: &std::path::Path) -> Source {
    eprintln!("this hushphone was built without the mesh; build it with --features mesh");
    std::process::exit(2)
}
