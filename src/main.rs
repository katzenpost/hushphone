use std::path::PathBuf;

use hushphone::Setup;
use hushphone::backend::Source;

const USAGE: &str = "usage: hushphone [--daemon ADDRESS] [--sim SOCKET] [--memory] [--data DIR] [--name NAME]

  --daemon ADDRESS  kpclientd's thin client socket: @name, a path, or host:port (default @katzenpost)
  --sim SOCKET      use pigeonhole-sim at SOCKET instead of the mixnet
  --memory          use an in-memory Pigeonhole: nothing leaves this process
  --data DIR        keep state in DIR (default $XDG_DATA_HOME/hushphone)
  --name NAME       your name, on the first start";

fn main() {
    let mut daemon = None;
    let mut source = None;
    let mut data = None;
    let mut name = None;
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
            "--name" => name = Some(value()),
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
    let default_daemon = "@katzenpost".to_string();
    let setup = Setup {
        data_dir,
        cache_dir,
        source: source.unwrap_or_else(|| Source::Daemon(daemon.clone().unwrap_or_default())),
        default_daemon: daemon.unwrap_or(default_daemon),
        name,
    };
    if let Err(e) = hushphone::run(setup) {
        eprintln!("hushphone: {e}");
        std::process::exit(1);
    }
}
