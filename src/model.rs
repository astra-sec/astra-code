use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Harness {
    Codex,
    Claude,
    Pi,
    OpenCode,
}

impl Harness {
    pub const ALL: [Self; 4] = [Self::Codex, Self::Claude, Self::Pi, Self::OpenCode];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
        }
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Harness {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "codex" => Ok(Self::Codex),
            "claude" | "claude-code" => Ok(Self::Claude),
            "pi" => Ok(Self::Pi),
            "opencode" | "open-code" => Ok(Self::OpenCode),
            _ => Err(format!(
                "unknown harness {value:?}; expected codex, claude, pi, or opencode"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiProtocol {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
}

impl ApiProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai-responses",
            Self::OpenAiChatCompletions => "openai-chat-completions",
            Self::AnthropicMessages => "anthropic-messages",
        }
    }
}

impl fmt::Display for ApiProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ApiProtocol {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "openai-responses" | "responses" => Ok(Self::OpenAiResponses),
            "openai-chat-completions" | "openai-completions" | "chat-completions" => {
                Ok(Self::OpenAiChatCompletions)
            }
            "anthropic-messages" | "anthropic" | "messages" => Ok(Self::AnthropicMessages),
            _ => Err(format!(
                "unknown API protocol {value:?}; expected openai-responses, \
                 openai-chat-completions, or anthropic-messages"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Profile {
    #[default]
    Safe,
    Pentest,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Pentest => "pentest",
        }
    }
}

impl FromStr for Profile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "safe" => Ok(Self::Safe),
            "pentest" => Ok(Self::Pentest),
            _ => Err(format!(
                "unknown profile {value:?}; expected safe or pentest"
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub enum SecretSource {
    Env(String),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub enum PromptSource {
    Inline(String),
    File(PathBuf),
    Stdin,
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub harness: Harness,
    pub api: ApiProtocol,
    pub base_url: String,
    pub model: String,
    pub token: SecretSource,
    pub prompt: PromptSource,
    pub workspace: PathBuf,
    pub output: Option<PathBuf>,
    pub image: String,
    pub timeout_seconds: u64,
    pub profile: Profile,
    pub network: String,
    pub read_only_workspace: bool,
    pub keep_container: bool,
    pub dry_run: bool,
    pub dns: Vec<String>,
    pub dns_tcp: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job {
    pub harness: Harness,
    pub api: ApiProtocol,
    pub base_url: String,
    pub model: String,
    pub token: String,
    pub prompt: String,
}

pub fn validate_pair(harness: Harness, api: ApiProtocol) -> Result<(), String> {
    match harness {
        Harness::Claude if api != ApiProtocol::AnthropicMessages => {
            Err("claude currently requires --api anthropic-messages".to_owned())
        }
        _ => Ok(()),
    }
}
