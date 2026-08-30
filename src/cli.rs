//! Argument parsing, `--help`, exit codes. Replaces `clap`.

use std::path::PathBuf;

/// Bad usage. Distinct from 1 (runtime failure) so scripts can tell them apart.
pub const EXIT_USAGE: i32 = 2;

pub struct Args {
    pub root: PathBuf,
    pub port: u16,
    /// Overrides the address printed in the URL. For the no-default-route case.
    pub host: Option<String>,
    pub no_index: bool,
    /// Inverts the terminal QR rendering for light-themed terminals (rung 5).
    pub invert: bool,
}

pub enum Parsed {
    Run(Args),
    Help,
}

const USAGE: &str = "\
darkroom — point it at a folder of photos, browse them from your phone

USAGE:
    darkroom <FOLDER> [OPTIONS]

OPTIONS:
    --port <PORT>    Port to listen on            [default: 8080]
    --host <ADDR>    Address to print in the URL  [default: auto-detected]
    --no-index       Serve an existing index without re-scanning
    --invert         Invert the terminal QR code for light-themed terminals
    -h, --help       Print this message

EXAMPLES:
    darkroom ~/Pictures
    darkroom D:\\Photos --port 9000
";

pub fn print_help() {
    print!("{USAGE}");
}

pub fn parse<I: IntoIterator<Item = String>>(argv: I) -> Result<Parsed, String> {
    let mut root: Option<PathBuf> = None;
    let mut port: u16 = 8080;
    let mut host: Option<String> = None;
    let mut no_index = false;
    let mut invert = false;

    let mut it = argv.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--no-index" => no_index = true,
            "--invert" => invert = true,
            "--port" => {
                let v = it.next().ok_or("--port needs a value")?;
                port = v.parse().map_err(|_| format!("--port: `{v}` is not a port number"))?;
                if port == 0 {
                    return Err("--port: 0 asks the OS to pick, which makes the printed URL a lie".into());
                }
            }
            "--host" => host = Some(it.next().ok_or("--host needs a value")?),
            // A flag we do not know is a usage error, never a path. Silently
            // treating `--prot 9000` as a folder name is how a tool ends up
            // scanning the wrong directory without saying so.
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            other => {
                if root.is_some() {
                    return Err(format!("unexpected second folder `{other}`"));
                }
                root = Some(PathBuf::from(other));
            }
        }
    }

    let root = root.ok_or("no folder given — try `darkroom ~/Pictures`")?;
    Ok(Parsed::Run(Args { root, port, host, no_index, invert }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Result<Parsed, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn folder_is_required() {
        assert!(args(&[]).is_err());
    }

    #[test]
    fn parses_folder_and_port() {
        let Ok(Parsed::Run(a)) = args(&["/photos", "--port", "9000"]) else { panic!() };
        assert_eq!(a.root, PathBuf::from("/photos"));
        assert_eq!(a.port, 9000);
    }

    #[test]
    fn help_wins_over_everything() {
        assert!(matches!(args(&["/photos", "--help"]), Ok(Parsed::Help)));
    }

    #[test]
    fn unknown_flag_is_not_a_path() {
        assert!(args(&["--prot", "9000"]).is_err());
    }

    #[test]
    fn rejects_port_zero() {
        assert!(args(&["/photos", "--port", "0"]).is_err());
    }
}
