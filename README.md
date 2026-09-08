# astra-code

`astra-code` is a Rust CLI for running coding harnesses inside an `astra-kali`
container. It is an independent repository: it consumes the image but does not
share source code or a release lifecycle with `astra-kali`.

The same binary has two roles:

- on the host, it validates the request and starts Docker;
- in the container, the hidden `shim` command writes an ephemeral harness
  configuration and execs Codex, Claude Code, Pi, or OpenCode.

In API-token mode, the token and prompt travel through Docker stdin using a length-prefixed
protocol. They are not included in `docker run` arguments, labels, or environment
settings, so `docker inspect` does not expose them.
Codex can also reuse the host's native login with `--host-auth`.

## Requirements

- Linux with Docker Engine 20.10 or newer
- An `astra-kali` image containing the supported harness binaries
- Rust 1.85 or newer when building from source

## Build

```sh
cargo build --release
./target/release/astra-code doctor --image astra-kali:latest
```

`doctor` checks that the current Codex, Claude, Pi, and OpenCode executables
exist in the selected image. A `codex-chat` 0.80.0 binary is detected when
present but is optional; only the legacy Chat Completions path needs it.

## Run

Codex using the host's existing ChatGPT login:

```sh
codex login status
astra-code run \
  --harness codex \
  --host-auth \
  --model gpt-6-astra \
  --codex-effort ultra \
  --image astra-kali:codex-ultra \
  --workspace ./target-project \
  --prompt 'Inspect the tests, fix the bug, and verify the result.'
```

`--host-auth` finds the file-backed login at `$CODEX_HOME/auth.json`, falling
back to `$HOME/.codex/auth.json`. It shares only that file with the container,
with write access so Codex can persist refreshed credentials for subsequent
host and container runs. Codex 0.153.4 saves file credentials in place. The
container's configuration, cache and session state remain isolated. Credentials
are not copied into the job protocol, environment or run artifacts.

This mode uses Codex's built-in OpenAI provider and `file` credential storage.
A ChatGPT login therefore uses the native Codex service, without a third-party
API gateway. Host provider settings, plugins and MCP configuration are not
imported. File-backed API-key logins use the same native provider; keyring-only
credentials require a file-backed login first. Account access still determines
which models can run.

The option currently requires `--harness codex`. `--api` may be omitted or set
to `openai-responses`; `--base-url`, `--token-env` and `--token-file` cannot be
combined with `--host-auth`. Other harnesses retain the API-token mode below.

Codex against an OpenAI Responses-compatible gateway:

```sh
export MY_LLM_TOKEN='...'
astra-code run \
  --harness codex \
  --api openai-responses \
  --base-url https://gateway.example/v1 \
  --model gpt-5.4 \
  --token-env MY_LLM_TOKEN \
  --workspace ./target-project \
  --prompt 'Inspect the tests, fix the bug, and verify the result.'
```

For a Chat Completions-only gateway, change the protocol to
`--api openai-chat-completions`. The adapter automatically selects the isolated
legacy Codex binary. Current Codex releases no longer support that protocol.

Codex reasoning effort can be set explicitly with `--codex-effort`:

```sh
astra-code run ... --harness codex --api openai-responses \
  --model gpt-6-astra --codex-effort ultra
```

Supported values are `low`, `medium`, `high`, `xhigh`, `max`, and `ultra`.
The value is written to the isolated Codex configuration as
`model_reasoning_effort`; omitting it preserves the harness default. Both the
selected model and the Codex executable in the image must support the requested
level. For example, Codex 0.132.0 cannot parse `ultra`; a recent executable such
as 0.153.4 is required. Updating the host astra-code executable does not update
Codex inside the image. Prefer Responses for current models and effort levels.

To create the separate `astra-kali:codex-ultra` image used above from a compatible
Codex npm installation on the host (validated with 0.153.4):

```sh
(
  set -eu
  codex --version
  codex_image_context="$(mktemp -d)"
  cp -a "$(npm root -g)/@openai/codex" "$codex_image_context/codex"
  cat > "$codex_image_context/Dockerfile" <<'DOCKERFILE'
FROM astra-kali:latest
COPY --chown=0:0 codex/ /opt/astra-codex/
RUN chmod -R a+rX /opt/astra-codex && ln -sf /opt/astra-codex/bin/codex.js /usr/local/bin/codex
DOCKERFILE
  docker build -t astra-kali:codex-ultra "$codex_image_context"
  rm -r -- "$codex_image_context"
)
```

This copies the installed CLI package, including its platform binary. The image
must match the host architecture; user credentials and Codex home configuration
are supplied separately at runtime and are not part of the build context.

Claude Code against an Anthropic Messages-compatible gateway:

```sh
astra-code run \
  --harness claude \
  --api anthropic-messages \
  --base-url https://gateway.example \
  --model claude-sonnet-4-5 \
  --token-file ~/.config/my-gateway/token \
  --prompt-file task.md
```

Claude-specific execution controls are explicit and optional:

```sh
astra-code run ... \
  --claude-effort high \
  --claude-max-turns 100 \
  --claude-disallowed-tool WebSearch \
  --claude-disallowed-tool WebFetch
```

Codex, Pi, and OpenCode accept these OpenAI protocol values:

```text
openai-responses
openai-chat-completions
```

Pi and OpenCode additionally accept `anthropic-messages`. Claude requires it.

If neither `--prompt` nor `--prompt-file` is present, the prompt is read from
stdin. Use `--dry-run` to inspect the complete, redacted Docker command without
reading the token or starting a container.

An orchestrator can supply a stable Docker/artifact identifier with `--run-id`.
Run IDs must be Docker-name-safe. Extra host data can be exposed without write
access using repeatable `--read-only-mount HOST_PATH:CONTAINER_PATH` options.

## Networking

With the default bridge network, loopback base URLs such as
`http://127.0.0.1:8080/v1` are automatically rewritten to
`http://host.docker.internal:8080/v1`, and the Linux host-gateway mapping is
added. `--network host` disables that rewrite.

The DNS options are not required under normal conditions. On hosts where
Docker's UDP DNS is blocked, force DNS over TCP:

```sh
astra-code run ... --dns 223.5.5.5 --dns-tcp
```

## Profiles and artifacts

The default `safe` profile drops all Linux capabilities, enables
`no-new-privileges`, and normally runs as the host UID/GID. The `pentest`
profile normally runs as root and grants only `NET_RAW` and `NET_ADMIN`; it does
not mount the Docker socket and does not use `--privileged`.

Claude is the deliberate exception for both profiles: it rejects unattended
permission bypass under root, so astra-code runs it as the invoking caller's
non-root UID/GID. The mounted workspace and the run-scoped tmpfs/ephemeral HOME
are assigned to that same identity; no fixed image account or numeric UID is
required. A root caller must use a non-root service account when selecting the
Claude harness. Other harnesses retain the normal profile identity described
above.

The shim clears the harness environment and restores only a small runtime
allowlist from the image. This includes the image `PATH`, Playwright browser and
configuration variables, IDA wrapper variables, and Python runtime paths. LLM
credentials are still supplied only by astra-code itself.

Each run writes the following files in a mode `0700` directory. Files are
created with mode `0600`:

- `events.jsonl`: the harness's raw streaming JSON output;
- `stderr.log`: diagnostics from the shim and harness;
- `result.json`: status and non-secret run metadata.

The default directory is `./astra-code-runs/<run-id>`. Override it with
`--output`. Harness event streams can contain the full prompt and model output;
treat the artifact directory as sensitive.

## Security model

In API-token mode, the token and prompt are delivered to the container shim over stdin, so they do
not appear in `docker run` arguments, labels, or container configuration. The
token is provided only in the selected harness process environment. Some
harnesses receive the prompt as a child-process argument after the shim starts,
which can be observed by a privileged host or container process.

In host-auth mode, only the prompt uses stdin; Codex reads and refreshes the
shared host `auth.json` file directly. The Docker command exposes that file's
path, not its contents. The mount is writable to preserve native token refresh;
the runner does not copy credentials into its output directory or protocol.

The default `safe` profile is intended to reduce accidental host impact, not to
make an untrusted image safe. A host administrator and a container process with
sufficient privileges can inspect process memory, arguments, mounted workspace
files, and network traffic. Do not mount the Docker socket or use untrusted
images with production credentials.

## Current scope

The contract remains one task per container, raw harness event streams and no
session resume. In API-token mode, no provider token is stored on disk;
ephemeral config files refer to a child-only environment variable. Host-auth
mode reuses the existing file-backed Codex login and allows native refresh to
update that file. The pinned legacy Codex binary is used only for Chat Completions and
does not receive current Codex security or feature updates. Prefer Responses
when the upstream gateway supports it.
