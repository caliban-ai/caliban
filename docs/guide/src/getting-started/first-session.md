# Your First Session

Get caliban answering questions in under five minutes.

## Set an API key

Caliban needs credentials for at least one provider before it can call a model. The quickest path is an environment variable. For Anthropic (the default provider):

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

For other providers, see [Configuring Providers & API Keys](../providers/configuration.md).

## Run a one-shot prompt

The `-p` / `--print` flag runs caliban non-interactively: it sends your prompt, streams the response to stdout, then exits.

```bash
caliban -p "What is the capital of France?"
```

That's it. The assistant's reply prints to stdout.

```admonish note title="Default provider and model"
When no `--provider` or `--model` flag is given, caliban defaults to **Anthropic** with model **`claude-sonnet-4-6`**. You can override either flag on the command line:

    caliban --provider openai --model gpt-5.5 -p "Hello"
```

## Work in a directory

Caliban uses the current working directory as the workspace root for file and shell tools. Just run it from your project:

```bash
cd ~/dev/my-project
caliban -p "Summarise README.md"
```

## Enter interactive mode

Drop the `-p` flag (and any prompt) to enter the interactive TUI instead:

```bash
caliban
```

Caliban detects that stdin is a TTY and launches the ratatui interface. Type your message and press Enter. To quit, press Ctrl-C or Ctrl-D at an empty prompt.

For a tour of the TUI, see [The Interactive TUI](./tui.md).

## Named sessions

Every conversation can be saved to a named session and resumed later:

```bash
# First run — creates a session called "research"
caliban --session research "Read README.md and summarise it"

# Later — resume the same conversation
caliban --resume research
```

Sessions are stored on disk under the platform's data directory (for example `~/.local/share/caliban/sessions/` on Linux). See [Sessions & Persistence](../interactive/sessions.md) for details.
