use crate::model::{ApiProtocol, Harness, Job, validate_pair};
use crate::protocol;
use crate::util::{json_string, toml_string, write_private};
use std::collections::BTreeMap;
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Invocation {
    program: &'static str,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    stdin: Option<Vec<u8>>,
}

pub fn run_shim() -> Result<i32, String> {
    let job = protocol::read_job(io::stdin().lock())?;
    validate_pair(job.harness, job.api)?;
    if job.token.is_empty() {
        return Err("API token is empty".to_owned());
    }

    let state_dir = PathBuf::from(
        env::var_os("ASTRA_CODE_STATE_DIR").unwrap_or_else(|| "/run/astra-code".into()),
    );
    std::fs::create_dir_all(&state_dir)
        .map_err(|e| format!("create shim state directory {}: {e}", state_dir.display()))?;
    let invocation = build_invocation(&job, &state_dir)?;

    eprintln!(
        "astra-code: starting {} with model {} ({})",
        job.harness, job.model, job.api
    );
    let mut command = Command::new(invocation.program);
    command
        .args(&invocation.args)
        .current_dir("/workspace")
        .env_clear()
        .envs(invocation.env)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(if invocation.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

    let mut child = command
        .spawn()
        .map_err(|e| format!("start {}: {e}", invocation.program))?;
    if let Some(input) = invocation.stdin {
        let mut stdin = child
            .stdin
            .take()
            .ok_or("harness stdin was not available")?;
        stdin
            .write_all(&input)
            .map_err(|e| format!("write prompt to harness: {e}"))?;
    }
    let status = child
        .wait()
        .map_err(|e| format!("wait for {}: {e}", invocation.program))?;
    Ok(status.code().unwrap_or(1))
}

fn build_invocation(job: &Job, state_dir: &Path) -> Result<Invocation, String> {
    match job.harness {
        Harness::Codex => codex(job, state_dir),
        Harness::Claude => claude(job, state_dir),
        Harness::Pi => pi(job, state_dir),
        Harness::OpenCode => opencode(job, state_dir),
    }
}

fn common_env(home: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "PATH".to_owned(),
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned(),
        ),
        ("HOME".to_owned(), home.display().to_string()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("CI".to_owned(), "true".to_owned()),
        ("TERM".to_owned(), "dumb".to_owned()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("DO_NOT_TRACK".to_owned(), "1".to_owned()),
    ])
}

fn codex(job: &Job, state_dir: &Path) -> Result<Invocation, String> {
    let (program, wire_api, ephemeral) = match job.api {
        ApiProtocol::OpenAiResponses => ("codex", "responses", true),
        ApiProtocol::OpenAiChatCompletions => ("codex-chat", "chat", false),
        ApiProtocol::AnthropicMessages => {
            return Err("codex does not support anthropic-messages".to_owned());
        }
    };
    let home = state_dir.join("home");
    let codex_home = state_dir.join("codex");
    let config = format!(
        "model = {}\n\
         model_provider = \"astra\"\n\
         approval_policy = \"never\"\n\
         sandbox_mode = \"danger-full-access\"\n\n\
         [model_providers.astra]\n\
         name = \"ASTRA custom provider\"\n\
         base_url = {}\n\
         env_key = \"ASTRA_API_TOKEN\"\n\
         wire_api = {}\n",
        toml_string(&job.model),
        toml_string(&job.base_url),
        toml_string(wire_api),
    );
    write_private(&codex_home.join("config.toml"), &config)?;
    let mut environment = common_env(&home);
    environment.insert("CODEX_HOME".to_owned(), codex_home.display().to_string());
    environment.insert("ASTRA_API_TOKEN".to_owned(), job.token.clone());

    let mut args = vec!["exec".to_owned(), "--json".to_owned()];
    if ephemeral {
        args.push("--ephemeral".to_owned());
    }
    args.extend([
        "--skip-git-repo-check".to_owned(),
        "--dangerously-bypass-approvals-and-sandbox".to_owned(),
        "-C".to_owned(),
        "/workspace".to_owned(),
        "-m".to_owned(),
        job.model.clone(),
        "-".to_owned(),
    ]);

    Ok(Invocation {
        program,
        args,
        env: environment,
        stdin: Some(job.prompt.as_bytes().to_vec()),
    })
}

fn claude(job: &Job, state_dir: &Path) -> Result<Invocation, String> {
    if job.api != ApiProtocol::AnthropicMessages {
        return Err("claude only supports anthropic-messages in astra-code".to_owned());
    }
    let home = state_dir.join("home");
    let mut environment = common_env(&home);
    environment.insert("ANTHROPIC_BASE_URL".to_owned(), job.base_url.clone());
    environment.insert("ANTHROPIC_API_KEY".to_owned(), job.token.clone());
    environment.insert("ANTHROPIC_AUTH_TOKEN".to_owned(), job.token.clone());
    environment.insert("DISABLE_TELEMETRY".to_owned(), "1".to_owned());
    environment.insert("DISABLE_ERROR_REPORTING".to_owned(), "1".to_owned());
    environment.insert("DISABLE_AUTOUPDATER".to_owned(), "1".to_owned());

    Ok(Invocation {
        program: "claude",
        args: vec![
            "--bare".to_owned(),
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
            "--no-session-persistence".to_owned(),
            "--dangerously-skip-permissions".to_owned(),
            "--model".to_owned(),
            job.model.clone(),
            job.prompt.clone(),
        ],
        env: environment,
        stdin: None,
    })
}

fn pi(job: &Job, state_dir: &Path) -> Result<Invocation, String> {
    let home = state_dir.join("home");
    let api = match job.api {
        ApiProtocol::OpenAiResponses => "openai-responses",
        ApiProtocol::OpenAiChatCompletions => "openai-completions",
        ApiProtocol::AnthropicMessages => "anthropic-messages",
    };
    let config = format!(
        "{{\"providers\":{{\"astra\":{{\"baseUrl\":{},\"api\":{},\
         \"apiKey\":\"$ASTRA_API_TOKEN\",\"models\":[{{\"id\":{},\"name\":{}}}]}}}}}}",
        json_string(&job.base_url),
        json_string(api),
        json_string(&job.model),
        json_string(&job.model),
    );
    write_private(&home.join(".pi/agent/models.json"), &config)?;
    let mut environment = common_env(&home);
    environment.insert("ASTRA_API_TOKEN".to_owned(), job.token.clone());
    environment.insert("PI_TELEMETRY".to_owned(), "0".to_owned());

    Ok(Invocation {
        program: "pi",
        args: vec![
            "-p".to_owned(),
            "--mode".to_owned(),
            "json".to_owned(),
            "--no-session".to_owned(),
            "--no-extensions".to_owned(),
            "--no-skills".to_owned(),
            "--no-prompt-templates".to_owned(),
            "--provider".to_owned(),
            "astra".to_owned(),
            "--model".to_owned(),
            job.model.clone(),
            job.prompt.clone(),
        ],
        env: environment,
        stdin: None,
    })
}

fn opencode(job: &Job, state_dir: &Path) -> Result<Invocation, String> {
    let home = state_dir.join("home");
    let config_home = state_dir.join("config");
    let config_path = state_dir.join("opencode.json");
    let npm = match job.api {
        ApiProtocol::OpenAiResponses => "@ai-sdk/openai",
        ApiProtocol::OpenAiChatCompletions => "@ai-sdk/openai-compatible",
        ApiProtocol::AnthropicMessages => "@ai-sdk/anthropic",
    };
    let mut config = String::from(
        "{\"$schema\":\"https://opencode.ai/config.json\",\"provider\":{\"astra\":{\"npm\":",
    );
    config.push_str(&json_string(npm));
    config.push_str(",\"name\":\"ASTRA\",\"options\":{\"baseURL\":");
    config.push_str(&json_string(&job.base_url));
    config.push_str(",\"apiKey\":\"{env:ASTRA_API_TOKEN}\"},\"models\":{");
    config.push_str(&json_string(&job.model));
    config.push_str(":{\"name\":");
    config.push_str(&json_string(&job.model));
    config.push_str("}}}}}");
    write_private(&config_path, &config)?;
    let mut environment = common_env(&home);
    environment.insert("ASTRA_API_TOKEN".to_owned(), job.token.clone());
    environment.insert(
        "XDG_CONFIG_HOME".to_owned(),
        config_home.display().to_string(),
    );
    environment.insert(
        "OPENCODE_CONFIG".to_owned(),
        config_path.display().to_string(),
    );

    Ok(Invocation {
        program: "opencode",
        args: vec![
            "run".to_owned(),
            "--pure".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--auto".to_owned(),
            "--model".to_owned(),
            format!("astra/{}", job.model),
            "--dir".to_owned(),
            "/workspace".to_owned(),
            job.prompt.clone(),
        ],
        env: environment,
        stdin: None,
    })
}

#[cfg(test)]
mod tests {
    use super::build_invocation;
    use crate::model::{ApiProtocol, Harness, Job};
    use std::path::Path;

    fn job(harness: Harness, api: ApiProtocol) -> Job {
        Job {
            harness,
            api,
            base_url: "https://gateway.example/v1".to_owned(),
            model: "example-model".to_owned(),
            token: "never-write-me".to_owned(),
            prompt: "inspect this project".to_owned(),
        }
    }

    #[test]
    fn codex_chat_uses_legacy_binary() {
        let temporary =
            std::env::temp_dir().join(format!("astra-code-test-{}", std::process::id()));
        let invocation = build_invocation(
            &job(Harness::Codex, ApiProtocol::OpenAiChatCompletions),
            &temporary,
        )
        .unwrap();
        assert_eq!(invocation.program, "codex-chat");
        assert!(!invocation.args.iter().any(|arg| arg == "--ephemeral"));
        let config = std::fs::read_to_string(temporary.join("codex/config.toml")).unwrap();
        assert!(config.contains("wire_api = \"chat\""));
        let _ = std::fs::remove_dir_all(temporary);
    }

    #[test]
    fn token_is_not_in_pi_arguments() {
        let temporary =
            std::env::temp_dir().join(format!("astra-code-test-pi-{}", std::process::id()));
        let invocation = build_invocation(
            &job(Harness::Pi, ApiProtocol::OpenAiResponses),
            Path::new(&temporary),
        )
        .unwrap();
        assert!(
            !invocation
                .args
                .iter()
                .any(|arg| arg.contains("never-write-me"))
        );
        let _ = std::fs::remove_dir_all(temporary);
    }
}
