use gittree_relay_probe::{HttpRelayProbeClient, RelayProbeError, probe_relay};

struct ProbeCli {
    relay: String,
    json: bool,
}

impl ProbeCli {
    fn parse<I, T>(args: I) -> Result<Self, RelayProbeError>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString>,
    {
        let mut relay: Option<String> = None;
        let mut json = false;

        let mut iter = args.into_iter().map(|arg| arg.into().to_string_lossy().to_string());
        iter.next();
        while let Some(value) = iter.next() {
            match value.as_str() {
                "--json" => json = true,
                "--relay" => {
                    let Some(next) = iter.next() else {
                        return Err(RelayProbeError::InvalidRelayUrl("--relay".to_string()));
                    };
                    relay = Some(next);
                }
                _ if value.starts_with("--relay=") => {
                    relay = Some(value.trim_start_matches("--relay=").to_string());
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => {
                    return Err(RelayProbeError::InvalidRelayUrl(format!(
                        "unknown flag {other}"
                    )));
                }
            }
        }

        let relay = relay.ok_or_else(|| RelayProbeError::InvalidRelayUrl("missing --relay".into()))?;
        Ok(Self { relay, json })
    }
}

fn print_help() {
    println!(
        "gittree-relay-probe --relay <wss://relay> [--json]\n\nFlags:\n  --relay <url>  Relay URL to probe\n  --json         Output JSON report\n  -h, --help     Show help\n"
    );
}

fn main() {
    let cli = match ProbeCli::parse(std::env::args_os()) {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("relay probe failed: {err}");
            std::process::exit(1);
        }
    };

    let client = match HttpRelayProbeClient::new() {
        Ok(client) => client,
        Err(err) => {
            eprintln!("relay probe failed: {err}");
            std::process::exit(1);
        }
    };

    match probe_relay(&cli.relay, &client) {
        Ok(result) => {
            if cli.json {
                let json = serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string());
                println!("{json}");
            } else {
                println!("relay: {}", result.relay_url);
                println!("compatible: {}", result.report.is_compatible());
                if !result.report.missing_required.is_empty() {
                    println!("missing required: {:?}", result.report.missing_required);
                }
                if !result.report.missing_optional.is_empty() {
                    println!("missing optional: {:?}", result.report.missing_optional);
                }
            }
        }
        Err(err) => {
            eprintln!("relay probe failed: {err}");
            std::process::exit(1);
        }
    }
}
