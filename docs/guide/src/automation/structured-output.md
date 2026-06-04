# Structured Output

`--json-schema` tells caliban to force the assistant's final reply into a JSON shape that matches a given schema. This is useful when a downstream script needs a machine-readable payload rather than freeform prose — a CI gate that needs a structured pass/fail verdict, a code-generation pipeline that expects a specific object shape, or any tool that would otherwise parse the reply with fragile string matching.

## Supplying a schema

```bash
--json-schema <FILE_OR_JSON>
```

The argument is either:

- A path to a `.json` file: `--json-schema ./schema.json`
- Inline JSON (detected when the value starts with `{` or `[`): `--json-schema '{"type":"object","required":["ok","message"]}'`

## What caliban does

1. Runs the agent loop normally.
2. After the final assistant turn, scans the reply for a balanced `{...}` JSON object. If the whole reply is valid JSON it is used as-is; otherwise the first balanced `{...}` block is extracted.
3. Validates the extracted object against the schema (required fields present, top-level and per-property types match).
4. On success: the validated object appears in the `structured_output` field of the `result` frame, and the process exits 0.
5. On failure: the `result` frame has `subtype: "error"` and the validation message appears in `error`. The process exits 2.

```admonish note title="Validation scope"
The built-in validator checks `required` fields and top-level `type` / per-property `type` constraints. It does not implement the full JSON Schema specification (no `$ref`, `oneOf`, `pattern`, etc.). Native provider-level structured output via the model router is planned and will extend coverage when available.
```

## Worked example

Suppose you want caliban to report whether a repository's tests pass, in a structured format.

**schema.json**

```json
{
  "type": "object",
  "required": ["passed", "summary"],
  "properties": {
    "passed": {"type": "boolean"},
    "summary": {"type": "string"},
    "failure_count": {"type": "integer"}
  }
}
```

**Invocation**

```bash
caliban \
  --output-format json \
  --json-schema ./schema.json \
  --bare \
  -p "Run the test suite and tell me whether it passed. Reply only with JSON."
```

**Successful result frame (stdout)**

```json
{
  "type": "result",
  "subtype": "success",
  "result": "{\"passed\": true, \"summary\": \"42 tests passed, 0 failed\", \"failure_count\": 0}",
  "session_id": "...",
  "total_cost_usd": 0.0021,
  "turns": 2,
  "total_input_tokens": 5400,
  "total_output_tokens": 310,
  "structured_output": {
    "passed": true,
    "summary": "42 tests passed, 0 failed",
    "failure_count": 0
  }
}
```

Read `structured_output` in your script:

```bash
result=$(caliban --output-format json --json-schema schema.json --bare \
           -p "Run tests and reply with JSON.")
passed=$(echo "$result" | jq '.structured_output.passed')
if [ "$passed" != "true" ]; then
  echo "Tests failed"
  exit 1
fi
```

**Failed validation (exit 2)**

```json
{
  "type": "result",
  "subtype": "error",
  "error": "missing required field `passed`",
  "session_id": "...",
  "total_cost_usd": 0.0018,
  "turns": 1,
  "total_input_tokens": 4800,
  "total_output_tokens": 95,
  "last_assistant_text": "All tests passed."
}
```

## Tips

- Instruct the model to reply **only** with JSON in your prompt. Models that wrap their answer in prose (e.g. "Here is the result: `{...}`") are handled — caliban scans for the first balanced `{...}` — but pure JSON replies validate more reliably.
- Combine with `--bare` to skip skills and hooks that might inject extra text into the reply.
- In stream-json mode, the `structured_output` field appears in the final `result` frame the same as in `json` mode.

## Related pages

- [Print Mode](./print-mode.md) — output formats and exit codes
- [CI Patterns](./ci.md) — complete pipeline recipes using structured output
