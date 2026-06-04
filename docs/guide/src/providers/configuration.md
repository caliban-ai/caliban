# Configuring Providers & API Keys

Caliban needs to know which provider to use and how to authenticate with it. Provider selection happens on the command line; authentication is supplied via environment variables or a dynamic key helper.

## Selecting a provider

Pass `--provider` to select the backend for a session:

```bash
caliban --provider anthropic   # default
caliban --provider openai
caliban --provider google
caliban --provider ollama      # no API key needed
```

When `--provider` is omitted, caliban resolves the provider from `settings.model` (see [Model Selection](./models.md)), falling back to `anthropic`.

## API key environment variables

Each provider reads its key from a well-known environment variable:

| Provider | Required env var | Optional env vars |
|---|---|---|
| Anthropic | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL`, `ANTHROPIC_VERSION` |
| OpenAI | `OPENAI_API_KEY` | `OPENAI_BASE_URL`, `OPENAI_ORG_ID`, `OPENAI_PROJECT` |
| Google | `GEMINI_API_KEY` | `GOOGLE_GEMINI_API_KEY` (alias), `GEMINI_BASE_URL`, `GEMINI_API_VERSION` |
| Ollama | *(none)* | `OLLAMA_BASE_URL` (default: `http://localhost:11434`) |
| Azure OpenAI | `AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_RESOURCE` | `AZURE_OPENAI_API_VERSION` (default: `2024-10-21`) |

Set the variable in your shell profile or pass it inline:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
caliban "summarize this file"
```

## Dynamic key helper (`api_key_helper`)

For secrets stored in a keychain, vault, or SSO-backed credential store, set `api_key_helper` in your settings file instead of exposing keys in the environment. The helper is a process caliban spawns to retrieve the current key on demand.

### Forms

**Bare string** — a single executable path or command string, used for all providers:

```toml
api_key_helper = "/usr/local/bin/get-caliban-key"
```

**Object** — one helper with explicit options:

```toml
[api_key_helper]
command = "/usr/local/bin/get-caliban-key"
provider = "anthropic"        # omit for wildcard ("*")
refreshIntervalMs = 300000    # 5 minutes (default)
slowHelperWarningMs = 10000   # warn if script takes > 10 s (default)
```

**Array** — different helpers per provider, with a wildcard fallback:

```toml
[[api_key_helper]]
provider = "anthropic"
command = "/usr/local/bin/anthropic-key"

[[api_key_helper]]
provider = "*"
command = "/usr/local/bin/generic-key"
```

The helper receives two environment variables:

- `CALIBAN_PROVIDER` — the provider id (e.g. `anthropic`)
- `CALIBAN_API_KEY_HELPER_TTL_MS` — the configured refresh interval in milliseconds

It must print the API key to stdout (trailing newline is stripped) and exit 0. Any non-zero exit is treated as an error.

### Caching and refresh

Caliban caches the returned key in memory for `refreshIntervalMs` (default 5 minutes). On a 401 or 403 from the provider, the cache entry is invalidated and the helper is re-invoked immediately for a fresh key. Override the TTL globally with `CALIBAN_API_KEY_HELPER_TTL_MS`.

```admonish tip title="Keyring integration"
A one-liner shell wrapper around `security find-generic-password` (macOS) or `secret-tool lookup` (Linux/GNOME) makes `api_key_helper` work with the OS keychain without storing the key in any file.
```

## Bedrock and Vertex configuration

AWS Bedrock and Google Vertex are configured through the [model router](./router.md) using `[provider.bedrock]` and `[provider.vertex]` blocks in `caliban.toml`. Authentication follows each platform's standard credential chain:

- **Bedrock** — AWS credential chain (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, instance profiles, `~/.aws/credentials`). A background task refreshes credentials on a configurable interval (default 5 minutes).
- **Vertex (Anthropic)** — Google Application Default Credentials (`GOOGLE_APPLICATION_CREDENTIALS`, `gcloud auth application-default login`).
- **Vertex (Google)** — same GCP ADC path as the Anthropic Vertex transport.

See [The Model Router](./router.md) for the full `caliban.toml` syntax, including `[provider.X]` blocks that let you override the env var name or base URL per provider.

For a full listing of every setting key, see [Settings Reference](../configuration/reference.md).
