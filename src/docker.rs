use crate::model::{Harness, Job, Profile, PromptSource, ReadOnlyMount, RunOptions, SecretSource};
use crate::protocol;
use crate::util::{
    create_private_dir, create_private_file, json_string, shell_join, unix_timestamp, write_private,
};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
const CLAUDE_UID: u32 = 1000;
const CLAUDE_GID: u32 = 1000;

pub fn run(options: RunOptions) -> Result<i32, String> {
    let workspace = options
        .workspace
        .canonicalize()
        .map_err(|e| format!("open workspace {}: {e}", options.workspace.display()))?;
    if !workspace.is_dir() {
        return Err(format!(
            "workspace is not a directory: {}",
            workspace.display()
        ));
    }

    let read_only_mounts = resolve_read_only_mounts(&options.read_only_mounts)?;
    let executable = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|e| format!("locate astra-code executable: {e}"))?;
    let run_id = options
        .run_id
        .clone()
        .unwrap_or_else(|| format!("astra-code-{}-{}", unix_timestamp(), std::process::id()));
    let (base_url, needs_host_gateway) = container_base_url(&options.base_url, &options.network);
    let args = docker_args(
        &options,
        &workspace,
        &executable,
        &run_id,
        &base_url,
        needs_host_gateway,
        &read_only_mounts,
    );

    if options.dry_run {
        println!("{}", shell_join("docker", &args));
        println!("# job credentials and prompt are sent over stdin; token is redacted");
        if base_url != options.base_url {
            println!("# base URL inside the container: {base_url}");
        }
        return Ok(0);
    }

    let token = read_secret(&options.token)?;
    if token.is_empty() {
        return Err("token source contained an empty value".to_owned());
    }
    let prompt = read_prompt(&options.prompt)?;
    if prompt.trim().is_empty() {
        return Err("prompt cannot be empty".to_owned());
    }
    let job = Job {
        harness: options.harness,
        api: options.api,
        base_url,
        model: options.model.clone(),
        token,
        prompt,
        claude: options.claude.clone(),
    };

    let output = options
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from("astra-code-runs").join(&run_id));
    create_private_dir(&output)?;
    let events = create_private_file(&output.join("events.jsonl"))?;
    let errors = create_private_file(&output.join("stderr.log"))?;

    install_signal_handlers();
    let started_at = unix_timestamp();
    eprintln!(
        "astra-code: run {run_id}; harness {}; artifacts {}",
        options.harness,
        output.display()
    );
    let mut child = Command::new("docker")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start docker: {e}"))?;

    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| "docker stdin was not available".to_owned())
        .and_then(|mut stdin| protocol::write_job(&mut stdin, &job));
    drop(job);
    if let Err(error) = write_result {
        let _ = stop_container(&run_id);
        let _ = child.wait();
        return Err(error);
    }

    let stdout = child
        .stdout
        .take()
        .ok_or("docker stdout was not available")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("docker stderr was not available")?;
    let stdout_thread = thread::spawn(move || copy_stream(stdout, io::stdout(), events));
    let stderr_thread = thread::spawn(move || copy_stream(stderr, io::stderr(), errors));

    let outcome = wait_for_child(
        &mut child,
        Duration::from_secs(options.timeout_seconds),
        &run_id,
    )?;
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    let (status, exit_code) = match outcome {
        WaitOutcome::Exited(0) => ("success", 0),
        WaitOutcome::Exited(code) => ("failed", code),
        WaitOutcome::TimedOut => ("timeout", 124),
        WaitOutcome::Interrupted => ("interrupted", 130),
    };
    write_result_file(
        &output,
        &run_id,
        status,
        exit_code,
        started_at,
        unix_timestamp(),
        &options,
    )?;
    eprintln!("astra-code: {status}; exit code {exit_code}");
    Ok(exit_code)
}

pub fn doctor(image: &str) -> Result<i32, String> {
    let script = "set -eu; test \"$(id -u kali)\" = 1000; test \"$(id -g kali)\" = 1000; \
                  codex --version; claude --version; pi --version; opencode --version; \
                  if command -v codex-chat >/dev/null 2>&1; then codex-chat --version; \
                  else echo 'codex-chat: not installed (optional legacy Chat Completions adapter)'; fi";
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "/bin/bash",
            image,
            "-lc",
            script,
        ])
        .status()
        .map_err(|e| format!("start docker: {e}"))?;
    let root_code = status.code().unwrap_or(1);
    if root_code != 0 {
        return Ok(root_code);
    }

    let non_root_script = "set -eu; test \"$(id -u)\" = 1000; test \"$(id -g)\" = 1000; \
                           test \"$HOME\" = /home/kali; codex --version; claude --version; \
                           pi --version; opencode --version; playwright-cli --version";
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--user",
            "1000:1000",
            "--workdir",
            "/home/kali",
            "--entrypoint",
            "/bin/bash",
            image,
            "-lc",
            non_root_script,
        ])
        .status()
        .map_err(|e| format!("start non-root docker doctor: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

fn docker_args(
    options: &RunOptions,
    workspace: &Path,
    executable: &Path,
    run_id: &str,
    base_url: &str,
    needs_host_gateway: bool,
    read_only_mounts: &[ReadOnlyMount],
) -> Vec<String> {
    let ids = runtime_ids(options.profile, options.harness, effective_ids());
    let tmpfs = match ids {
        Some((uid, gid)) => {
            format!("/run/astra-code:rw,noexec,nosuid,nodev,mode=0700,uid={uid},gid={gid}")
        }
        None => "/run/astra-code:rw,noexec,nosuid,nodev,mode=0700".to_owned(),
    };
    let mut args = vec![
        "run".to_owned(),
        "-i".to_owned(),
        "--init".to_owned(),
        "--name".to_owned(),
        run_id.to_owned(),
    ];
    if !options.keep_container {
        args.push("--rm".to_owned());
    }
    args.extend([
        "--workdir".to_owned(),
        "/workspace".to_owned(),
        "--network".to_owned(),
        options.network.clone(),
        "--mount".to_owned(),
        format!(
            "type=bind,src={},dst=/workspace{}",
            workspace.display(),
            if options.read_only_workspace {
                ",readonly"
            } else {
                ""
            }
        ),
        "--mount".to_owned(),
        format!(
            "type=bind,src={},dst=/usr/local/bin/astra-code,readonly",
            executable.display()
        ),
        "--tmpfs".to_owned(),
        tmpfs,
    ]);

    match options.profile {
        Profile::Safe => {
            args.extend([
                "--cap-drop".to_owned(),
                "ALL".to_owned(),
                "--security-opt".to_owned(),
                "no-new-privileges".to_owned(),
                "--pids-limit".to_owned(),
                "2048".to_owned(),
            ]);
        }
        Profile::Pentest => {
            args.extend([
                "--security-opt".to_owned(),
                "no-new-privileges".to_owned(),
                "--cap-add".to_owned(),
                "NET_RAW".to_owned(),
                "--cap-add".to_owned(),
                "NET_ADMIN".to_owned(),
            ]);
        }
    }
    if let Some((uid, gid)) = ids {
        args.extend(["--user".to_owned(), format!("{uid}:{gid}")]);
    }

    for mount in read_only_mounts {
        args.extend([
            "--mount".to_owned(),
            format!(
                "type=bind,src={},dst={},readonly",
                mount.source.display(),
                mount.target
            ),
        ]);
    }

    for dns in &options.dns {
        args.extend(["--dns".to_owned(), dns.clone()]);
    }
    if options.dns_tcp {
        args.extend(["--dns-opt".to_owned(), "use-vc".to_owned()]);
    }
    if needs_host_gateway {
        args.extend([
            "--add-host".to_owned(),
            "host.docker.internal:host-gateway".to_owned(),
        ]);
    }

    args.extend([
        "--label".to_owned(),
        "io.astra-code.managed=true".to_owned(),
        "--label".to_owned(),
        format!("io.astra-code.harness={}", options.harness),
        "--entrypoint".to_owned(),
        "/usr/local/bin/astra-code".to_owned(),
        options.image.clone(),
        "shim".to_owned(),
    ]);
    debug_assert!(!args.iter().any(|arg| arg == base_url));
    args
}

fn resolve_read_only_mounts(mounts: &[ReadOnlyMount]) -> Result<Vec<ReadOnlyMount>, String> {
    mounts
        .iter()
        .map(|mount| {
            let source = mount.source.canonicalize().map_err(|error| {
                format!(
                    "open read-only mount source {}: {error}",
                    mount.source.display()
                )
            })?;
            Ok(ReadOnlyMount {
                source,
                target: mount.target.clone(),
            })
        })
        .collect()
}

fn read_secret(source: &SecretSource) -> Result<String, String> {
    let value = match source {
        SecretSource::Env(name) => std::env::var(name)
            .map_err(|_| format!("token environment variable {name:?} is not set or not UTF-8"))?,
        SecretSource::File(path) => fs::read_to_string(path)
            .map_err(|e| format!("read token file {}: {e}", path.display()))?,
    };
    Ok(value.trim_end_matches(['\r', '\n']).to_owned())
}

fn read_prompt(source: &PromptSource) -> Result<String, String> {
    match source {
        PromptSource::Inline(value) => Ok(value.clone()),
        PromptSource::File(path) => fs::read_to_string(path)
            .map_err(|e| format!("read prompt file {}: {e}", path.display())),
        PromptSource::Stdin => {
            let mut value = String::new();
            io::stdin()
                .read_to_string(&mut value)
                .map_err(|e| format!("read prompt from stdin: {e}"))?;
            Ok(value)
        }
    }
}

fn container_base_url(base_url: &str, network: &str) -> (String, bool) {
    if network == "host" {
        return (base_url.to_owned(), false);
    }
    for host in ["localhost", "127.0.0.1", "[::1]"] {
        for scheme in ["http://", "https://"] {
            let prefix = format!("{scheme}{host}");
            if base_url == prefix
                || base_url.starts_with(&format!("{prefix}:"))
                || base_url.starts_with(&format!("{prefix}/"))
            {
                return (
                    format!("{scheme}host.docker.internal{}", &base_url[prefix.len()..]),
                    true,
                );
            }
        }
    }
    (base_url.to_owned(), false)
}

fn copy_stream(mut reader: impl Read, mut terminal: impl Write, mut file: File) {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                let chunk = &buffer[..length];
                let _ = terminal.write_all(chunk);
                let _ = terminal.flush();
                let _ = file.write_all(chunk);
            }
            Err(_) => break,
        }
    }
}

enum WaitOutcome {
    Exited(i32),
    TimedOut,
    Interrupted,
}

fn wait_for_child(
    child: &mut Child,
    timeout: Duration,
    run_id: &str,
) -> Result<WaitOutcome, String> {
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("wait for docker: {e}"))?
        {
            return Ok(WaitOutcome::Exited(status.code().unwrap_or(1)));
        }
        if INTERRUPTED.load(Ordering::Relaxed) {
            eprintln!("astra-code: interrupt received; stopping container");
            if let Err(error) = stop_container(run_id) {
                eprintln!("astra-code: warning: {error}");
            }
            let _ = child.wait();
            return Ok(WaitOutcome::Interrupted);
        }
        if started.elapsed() >= timeout {
            eprintln!("astra-code: timeout reached; stopping container");
            if let Err(error) = stop_container(run_id) {
                eprintln!("astra-code: warning: {error}");
            }
            let _ = child.wait();
            return Ok(WaitOutcome::TimedOut);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn stop_container(run_id: &str) -> Result<(), String> {
    let status = Command::new("docker")
        .args(["stop", "--timeout", "5", run_id])
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("stop container {run_id}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("docker could not stop container {run_id}"))
    }
}

#[allow(clippy::too_many_arguments)]
fn write_result_file(
    output: &Path,
    run_id: &str,
    status: &str,
    exit_code: i32,
    started_at: u64,
    finished_at: u64,
    options: &RunOptions,
) -> Result<(), String> {
    let document = format!(
        "{{\n  \"run_id\": {},\n  \"status\": {},\n  \"exit_code\": {},\n  \
         \"harness\": {},\n  \"api\": {},\n  \"model\": {},\n  \"image\": {},\n  \
         \"profile\": {},\n  \"started_at\": {},\n  \"finished_at\": {}\n}}\n",
        json_string(run_id),
        json_string(status),
        exit_code,
        json_string(options.harness.as_str()),
        json_string(options.api.as_str()),
        json_string(&options.model),
        json_string(&options.image),
        json_string(options.profile.as_str()),
        started_at,
        finished_at,
    );
    write_private(&output.join("result.json"), &document)
}

#[cfg(unix)]
fn effective_ids() -> Option<(u32, u32)> {
    unsafe extern "C" {
        fn geteuid() -> u32;
        fn getegid() -> u32;
    }
    Some(unsafe { (geteuid(), getegid()) })
}

fn runtime_ids(
    profile: Profile,
    harness: Harness,
    host_ids: Option<(u32, u32)>,
) -> Option<(u32, u32)> {
    match (profile, harness) {
        (_, Harness::Claude) => Some((CLAUDE_UID, CLAUDE_GID)),
        (Profile::Safe, _) => host_ids,
        (Profile::Pentest, _) => Some((0, 0)),
    }
}

#[cfg(not(unix))]
fn effective_ids() -> Option<(u32, u32)> {
    None
}

#[cfg(unix)]
fn install_signal_handlers() {
    extern "C" fn handle_signal(_: i32) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }
    unsafe extern "C" {
        fn signal(signal: i32, handler: usize) -> usize;
    }
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    unsafe {
        signal(SIGINT, handle_signal as *const () as usize);
        signal(SIGTERM, handle_signal as *const () as usize);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {}

#[cfg(test)]
mod tests {
    use super::{container_base_url, runtime_ids};
    use crate::model::{Harness, Profile};

    #[test]
    fn rewrites_loopback_for_bridge_network() {
        assert_eq!(
            container_base_url("http://127.0.0.1:8080/v1", "bridge"),
            ("http://host.docker.internal:8080/v1".to_owned(), true)
        );
    }

    #[test]
    fn leaves_loopback_alone_for_host_network() {
        assert_eq!(
            container_base_url("http://localhost:8080", "host"),
            ("http://localhost:8080".to_owned(), false)
        );
    }

    #[test]
    fn leaves_remote_url_alone() {
        assert_eq!(
            container_base_url("https://api.example/v1", "bridge"),
            ("https://api.example/v1".to_owned(), false)
        );
    }

    #[test]
    fn claude_always_uses_the_non_root_runtime_account() {
        assert_eq!(
            runtime_ids(Profile::Safe, Harness::Claude, Some((2000, 3000))),
            Some((1000, 1000))
        );
        assert_eq!(
            runtime_ids(Profile::Pentest, Harness::Claude, Some((2000, 3000))),
            Some((1000, 1000))
        );
    }

    #[test]
    fn other_harnesses_keep_the_profile_identity() {
        assert_eq!(
            runtime_ids(Profile::Safe, Harness::Codex, Some((2000, 3000))),
            Some((2000, 3000))
        );
        assert_eq!(
            runtime_ids(Profile::Pentest, Harness::Codex, Some((2000, 3000))),
            Some((0, 0))
        );
    }
}
