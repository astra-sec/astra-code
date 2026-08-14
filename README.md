# astra-code

`astra-code` is a Rust CLI for running coding harnesses inside an `astra-kali`
container. It is an independent repository: it consumes the image but does not
share source code or a release lifecycle with `astra-kali`.

The same binary has two roles:

- on the host, it validates the request and starts Docker;
- in the container, the hidden `shim` command writes an ephemeral harness
  configuration and execs Codex, Claude Code, Pi, or OpenCode.

The API token and prompt travel through Docker stdin using a length-prefixed
protocol. They are not included in `docker run` arguments, labels, or environment
settings, so `docker inspect` does not expose them.

## Requirements

- Linux with Docker Engine 20.10 or newer
- An `astra-kali` image containing the supported harness binaries
- Rust 1.85 or newer when building from source

## Build

```sh
cargo build --release
./target/release/astra-code doctor --image astra-kali:latest
```

`doctor` checks that all harness executables exist in the selected image. Codex
uses the current binary for Responses and a pinned `codex-chat` 0.80.0 binary
for the legacy Chat Completions protocol removed from current Codex releases.

## Run

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

Codex, Pi, and OpenCode accept these OpenAI protocol values:

```text
openai-responses
openai-chat-completions
```

Pi and OpenCode additionally accept `anthropic-messages`. Claude requires it.

If neither `--prompt` nor `--prompt-file` is present, the prompt is read from
stdin. Use `--dry-run` to inspect the complete, redacted Docker command without
reading the token or starting a container.

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
`no-new-privileges`, and runs as the host UID/GID. The `pentest` profile runs as
root and grants only `NET_RAW` and `NET_ADMIN`; it does not mount the Docker
socket and does not use `--privileged`.

Each run writes the following files in a mode `0700` directory. Files are
created with mode `0600`:

- `events.jsonl`: the harness's raw streaming JSON output;
- `stderr.log`: diagnostics from the shim and harness;
- `result.json`: status and non-secret run metadata.

The default directory is `./astra-code-runs/<run-id>`. Override it with
`--output`. Harness event streams can contain the full prompt and model output;
treat the artifact directory as sensitive.

## Security model

The token and prompt are delivered to the container shim over stdin, so they do
not appear in `docker run` arguments, labels, or container configuration. The
token is provided only in the selected harness process environment. Some
harnesses receive the prompt as a child-process argument after the shim starts,
which can be observed by a privileged host or container process.

The default `safe` profile is intended to reduce accidental host impact, not to
make an untrusted image safe. A host administrator and a container process with
sufficient privileges can inspect process memory, arguments, mounted workspace
files, and network traffic. Do not mount the Docker socket or use untrusted
images with production credentials.

## Current scope

This first version deliberately keeps the contract narrow: one task per
container, raw harness event streams, no session resume, and no provider token
stored on disk. Ephemeral config files refer to a child-only environment
variable. The pinned legacy Codex binary is used only for Chat Completions and
does not receive current Codex security or feature updates. Prefer Responses
when the upstream gateway supports it.
