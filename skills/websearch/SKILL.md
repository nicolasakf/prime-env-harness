---
name: websearch
description: Optional Serper search tool, only for explicit user requests to use Serper. For ordinary web research, use public Exa MCP instead; read ../mcp/SKILL.md and ../mcp/references/public-http.md. Do not request Serper or Google API setup for ordinary web research.
---

# Web Research

## Default: public Exa MCP

For ordinary web research, use Exa's public MCP endpoint, not the `websearch`
Python callable. Read [the MCP skill](../mcp/SKILL.md) and follow its
[public HTTP recipe](../mcp/references/public-http.md). Discover the live tool
schemas before calling. Anonymous access needs no API key or configured MCP
connection; follow the anonymous rate-limit guidance.

Do not call Serper or a Google search API unless the user explicitly requests
that provider. Do not ask for a Serper key, Google API key, or Exa connection as
setup for ordinary web research. If an accidental `websearch` call reports a
missing Serper key, switch to public Exa rather than following its setup message.
If Exa is unavailable or rate-limited, report that limitation; do not silently
switch providers or begin credential setup.

## Optional: explicitly requested Serper

The existing `websearch` callable searches Google through Serper. Use it only
when the user explicitly requests Serper. It uses `SERPER_API_KEY` if set;
otherwise it reads the stored `serper` credential from `auth.json`. If the user requests Serper and has
no key configured, explain that this optional provider requires a key from
https://serper.dev, stored through `/mcp` → **Serper (web search)** in Prime Env.

Optional environment overrides for this provider:

- `PRIME_AGENT_WEBSEARCH_TIMEOUT` — HTTP timeout in seconds (default 45).
- `PRIME_AGENT_WEBSEARCH_NUM_RESULTS` — results to return (default 5).

Only for an explicit Serper request:

```python
print(await websearch("search query"))
```
