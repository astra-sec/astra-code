use crate::model::{
    ApiProtocol, ClaudeOptions, Harness, Profile, PromptSource, ReadOnlyMount, RunOptions,
    SecretSource, validate_pair,
};
use std::env;
use std::path::PathBuf;
use std::str::FromStr;

pub enum Command {
    Run(Box<RunOptions>),
    Doctor { image: String },
    Harnesses,
    Shim,
    Help,
    Printed,
    Version,
}

pub fn parse() -> Result<Command, String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Ok(Command::Help);
    };

    match command.as_str() {
        "run" => parse_run(args.collect()),
        "doctor" => parse_doctor(args.collect()),
        "harnesses" => no_extra_args(args.collect(), Command::Harnesses),
        "shim" => no_extra_args(args.collect(), Command::Shim),
        "help" | "--help" | "-h" => Ok(Command::Help),
        "version" | "--version" | "-V" => Ok(Command::Version),
        _ => Err(format!(
            "unknown command {command:?}; run `astra-code help`"
        )),
    }
}

fn no_extra_args(args: Vec<String>, command: Command) -> Result<Command, String> {
    if args.is_empty() {
        Ok(command)
    } else {
        Err(format!("unexpected argument {:?}", args[0]))
    }
}

fn parse_doctor(args: Vec<String>) -> Result<Command, String> {
    let mut image = "astra-kali:latest".to_owned();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--image" => image = take_value(&args, &mut index, "--image")?,
            "--help" | "-h" => {
                println!("Usage: astra-code doctor [--image IMAGE]");
                return Ok(Command::Printed);
            }
            value => return Err(format!("unknown doctor option {value:?}")),
        }
        index += 1;
    }
    Ok(Command::Doctor { image })
}

fn parse_run(args: Vec<String>) -> Result<Command, String> {
    let mut harness = None;
    let mut api = None;
    let mut base_url = None;
    let mut model = None;
    let mut token = None;
    let mut prompt = None;
    let mut run_id = None;
    let mut workspace = env::current_dir().map_err(|e| format!("read current directory: {e}"))?;
    let mut output = None;
    let mut image = "astra-kali:latest".to_owned();
    let mut timeout_seconds = 3600;
    let mut profile = Profile::default();
    let mut network = "bridge".to_owned();
    let mut read_only_workspace = false;
    let mut keep_container = false;
    let mut dry_run = false;
    let mut dns = Vec::new();
    let mut dns_tcp = false;
    let mut read_only_mounts = Vec::new();
    let mut claude = ClaudeOptions::default();

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--harness" => {
                harness = Some(Harness::from_str(&take_value(
                    &args,
                    &mut index,
                    "--harness",
                )?)?)
            }
            "--api" => {
                api = Some(ApiProtocol::from_str(&take_value(
                    &args, &mut index, "--api",
                )?)?)
            }
            "--base-url" => base_url = Some(take_value(&args, &mut index, "--base-url")?),
            "--model" => model = Some(take_value(&args, &mut index, "--model")?),
            "--token-env" => set_once(
                &mut token,
                SecretSource::Env(take_value(&args, &mut index, "--token-env")?),
                "token source",
            )?,
            "--token-file" => set_once(
                &mut token,
                SecretSource::File(PathBuf::from(take_value(
                    &args,
                    &mut index,
                    "--token-file",
                )?)),
                "token source",
            )?,
            "--prompt" => set_once(
                &mut prompt,
                PromptSource::Inline(take_value(&args, &mut index, "--prompt")?),
                "prompt source",
            )?,
            "--prompt-file" => set_once(
                &mut prompt,
                PromptSource::File(PathBuf::from(take_value(
                    &args,
                    &mut index,
                    "--prompt-file",
                )?)),
                "prompt source",
            )?,
            "--run-id" => set_once(
                &mut run_id,
                take_value(&args, &mut index, "--run-id")?,
                "run ID",
            )?,
            "--workspace" => {
                workspace = PathBuf::from(take_value(&args, &mut index, "--workspace")?)
            }
            "--output" => output = Some(PathBuf::from(take_value(&args, &mut index, "--output")?)),
            "--image" => image = take_value(&args, &mut index, "--image")?,
            "--timeout" => {
                let raw = take_value(&args, &mut index, "--timeout")?;
                timeout_seconds = raw
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --timeout {raw:?}; expected seconds"))?;
                if timeout_seconds == 0 {
                    return Err("--timeout must be greater than zero".to_owned());
                }
            }
            "--profile" => {
                profile = Profile::from_str(&take_value(&args, &mut index, "--profile")?)?
            }
            "--network" => network = take_value(&args, &mut index, "--network")?,
            "--read-only-workspace" => read_only_workspace = true,
            "--keep-container" => keep_container = true,
            "--dry-run" => dry_run = true,
            "--dns" => dns.push(take_value(&args, &mut index, "--dns")?),
            "--dns-tcp" => dns_tcp = true,
            "--read-only-mount" => read_only_mounts.push(parse_read_only_mount(&take_value(
                &args,
                &mut index,
                "--read-only-mount",
            )?)?),
            "--claude-effort" => {
                let effort = take_value(&args, &mut index, "--claude-effort")?;
                if !matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max") {
                    return Err(format!(
                        "invalid --claude-effort {effort:?}; expected low, medium, high, xhigh, or max"
                    ));
                }
                set_once(&mut claude.effort, effort, "Claude effort")?;
            }
            "--claude-max-turns" => {
                let raw = take_value(&args, &mut index, "--claude-max-turns")?;
                let value = raw.parse::<u64>().map_err(|_| {
                    format!("invalid --claude-max-turns {raw:?}; expected an integer")
                })?;
                if value == 0 {
                    return Err("--claude-max-turns must be greater than zero".to_owned());
                }
                set_once(&mut claude.max_turns, value, "Claude max turns")?;
            }
            "--claude-allowed-tool" => claude.allowed_tools.push(require_tool_name(
                take_value(&args, &mut index, "--claude-allowed-tool")?,
                "--claude-allowed-tool",
            )?),
            "--claude-disallowed-tool" => claude.disallowed_tools.push(require_tool_name(
                take_value(&args, &mut index, "--claude-disallowed-tool")?,
                "--claude-disallowed-tool",
            )?),
            "--help" | "-h" => {
                print_run_help();
                return Ok(Command::Printed);
            }
            value => return Err(format!("unknown run option {value:?}")),
        }
        index += 1;
    }

    let harness = harness.ok_or("missing required --harness")?;
    let api = api.ok_or("missing required --api")?;
    validate_pair(harness, api)?;
    if harness != Harness::Claude && !claude.is_empty() {
        return Err("--claude-* options require --harness claude".to_owned());
    }

    let base_url = require_non_empty(base_url, "--base-url")?;
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err("--base-url must start with http:// or https://".to_owned());
    }
    let model = require_non_empty(model, "--model")?;
    let token = token.ok_or("missing token source; use --token-env or --token-file")?;
    if let Some(value) = run_id.as_deref() {
        validate_run_id(value)?;
    }

    Ok(Command::Run(Box::new(RunOptions {
        harness,
        api,
        base_url,
        model,
        token,
        prompt: prompt.unwrap_or(PromptSource::Stdin),
        run_id,
        workspace,
        output,
        image,
        timeout_seconds,
        profile,
        network,
        read_only_workspace,
        keep_container,
        dry_run,
        dns,
        dns_tcp,
        read_only_mounts,
        claude,
    })))
}

fn validate_run_id(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 128 {
        return Err("--run-id must contain between 1 and 128 characters".to_owned());
    }
    let mut chars = value.chars();
    let first = chars.next().expect("non-empty run ID");
    if !first.is_ascii_alphanumeric()
        || !chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        return Err(
            "--run-id must start with an ASCII letter or digit and contain only ASCII letters, digits, '.', '_', or '-'"
                .to_owned(),
        );
    }
    Ok(())
}

fn parse_read_only_mount(value: &str) -> Result<ReadOnlyMount, String> {
    let (source, target) = value.rsplit_once(':').ok_or_else(|| {
        format!("invalid --read-only-mount {value:?}; expected HOST_PATH:CONTAINER_PATH")
    })?;
    if source.is_empty() || !target.starts_with('/') || target == "/" {
        return Err(format!(
            "invalid --read-only-mount {value:?}; source must be non-empty and container path must be absolute and not '/'"
        ));
    }
    if source.contains([',', '\n', '\r']) || target.contains([',', '\n', '\r']) {
        return Err("--read-only-mount paths cannot contain commas or newlines".to_owned());
    }
    Ok(ReadOnlyMount {
        source: PathBuf::from(source),
        target: target.to_owned(),
    })
}

fn require_tool_name(value: String, option: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err(format!("{option} cannot be empty"))
    } else if value.contains(['\n', '\r']) {
        Err(format!("{option} cannot contain newlines"))
    } else {
        Ok(value)
    }
}

fn take_value(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn set_once<T>(slot: &mut Option<T>, value: T, label: &str) -> Result<(), String> {
    if slot.is_some() {
        Err(format!("{label} was specified more than once"))
    } else {
        *slot = Some(value);
        Ok(())
    }
}

fn require_non_empty(value: Option<String>, option: &str) -> Result<String, String> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(value),
        Some(_) => Err(format!("{option} cannot be empty")),
        None => Err(format!("missing required {option}")),
    }
}

pub fn print_help() {
    println!(
        "astra-code {version}\n\
         Run coding harnesses in an astra-kali container.\n\n\
         Usage:\n  \
           astra-code run [OPTIONS]\n  \
           astra-code doctor [--image IMAGE]\n  \
           astra-code harnesses\n  \
           astra-code help\n\n\
         Run `astra-code run --help` for run options.",
        version = env!("CARGO_PKG_VERSION")
    );
}

pub fn print_run_help() {
    println!(
        "Usage: astra-code run [OPTIONS]\n\n\
         Required:\n  \
           --harness NAME       codex, claude, pi, or opencode\n  \
           --api PROTOCOL       openai-responses, openai-chat-completions,\n  \
                                or anthropic-messages\n  \
           --base-url URL       API base URL\n  \
           --model MODEL        Provider model identifier\n  \
           --token-env NAME     Read token from a host environment variable\n  \
             or --token-file PATH\n\n\
         Prompt (stdin if omitted):\n  \
           --prompt TEXT\n  \
             or --prompt-file PATH\n\n\
         Container:\n  \
           --run-id ID          External run/container ID\n  \
           --workspace PATH     Directory mounted at /workspace (default: cwd)\n  \
           --image IMAGE        Default: astra-kali:latest\n  \
           --profile PROFILE    safe (default) or pentest\n  \
           --network NETWORK    Docker network mode (default: bridge)\n  \
           --read-only-workspace\n  \
           --read-only-mount HOST_PATH:CONTAINER_PATH\n  \
                                Repeatable extra read-only bind mount\n  \
           --dns SERVER         Repeatable Docker DNS setting\n  \
           --dns-tcp            Force Docker DNS over TCP\n\n\
         Execution:\n  \
           --timeout SECONDS    Default: 3600\n  \
           --output PATH        Run artifacts directory\n  \
           --keep-container     Do not pass --rm to Docker\n  \
           --dry-run            Print a redacted Docker command only\n\n\
         Claude only:\n  \
           --claude-effort LEVEL\n  \
           --claude-max-turns COUNT\n  \
           --claude-allowed-tool TOOL       Repeatable\n  \
           --claude-disallowed-tool TOOL    Repeatable"
    );
}

pub fn print_harnesses() {
    println!("HARNESS   SUPPORTED API PROTOCOLS");
    for harness in Harness::ALL {
        let protocols = match harness {
            Harness::Codex => "openai-responses, openai-chat-completions",
            Harness::Claude => "anthropic-messages",
            Harness::Pi | Harness::OpenCode => {
                "openai-responses, openai-chat-completions, anthropic-messages"
            }
        };
        println!("{:<9} {protocols}", harness.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_read_only_mount, take_value, validate_run_id};
    use std::path::PathBuf;

    #[test]
    fn takes_option_value() {
        let args = vec!["--model".to_owned(), "gpt-test".to_owned()];
        let mut index = 0;
        assert_eq!(
            take_value(&args, &mut index, "--model").unwrap(),
            "gpt-test"
        );
        assert_eq!(index, 1);
    }

    #[test]
    fn validates_external_run_ids() {
        assert!(validate_run_id("eval-20260829_123456-abcdef012345").is_ok());
        assert!(validate_run_id("../escape").is_err());
        assert!(validate_run_id("contains space").is_err());
    }

    #[test]
    fn parses_read_only_mounts() {
        let mount = parse_read_only_mount("/host/templates:/codex/templates/cve").unwrap();
        assert_eq!(mount.source, PathBuf::from("/host/templates"));
        assert_eq!(mount.target, "/codex/templates/cve");
        assert!(parse_read_only_mount("relative-only").is_err());
        assert!(parse_read_only_mount("/host:/").is_err());
    }
}
