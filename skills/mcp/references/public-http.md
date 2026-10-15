# Public Streamable HTTP MCP

Use this path for a public remote MCP endpoint that does not require a Prime Env
connection. Exa's public endpoint is the main example:

```python
import json
import httpx

EXA_MCP_URL = "https://mcp.exa.ai/mcp"


def _mcp_response_json(response):
    if response.status_code == 429:
        retry_after = response.headers.get("retry-after", "unknown")
        raise RuntimeError(f"MCP rate limit exceeded; Retry-After={retry_after}")
    response.raise_for_status()
    if "application/json" in response.headers.get("content-type", ""):
        return response.json()

    # Streamable HTTP commonly returns one or more Server-Sent Events.
    messages = [
        json.loads(line.removeprefix("data:").strip())
        for line in response.text.splitlines()
        if line.startswith("data:")
    ]
    if not messages:
        raise RuntimeError(f"MCP server returned no JSON message: {response.text[:500]}")
    return messages[-1]


async def public_mcp_request(url, method, params=None, request_headers=None):
    headers = {
        "Accept": "application/json, text/event-stream",
        "Content-Type": "application/json",
    }
    if request_headers:
        headers.update(request_headers)
    initialize = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "prime-env", "version": "1"},
        },
    }

    async with httpx.AsyncClient(timeout=45) as client:
        initialized = await client.post(url, headers=headers, json=initialize)
        initialized.raise_for_status()

        session_id = initialized.headers.get("mcp-session-id")
        if session_id:
            headers["Mcp-Session-Id"] = session_id
        headers["MCP-Protocol-Version"] = "2025-03-26"

        ready = await client.post(
            url,
            headers=headers,
            json={"jsonrpc": "2.0", "method": "notifications/initialized"},
        )
        ready.raise_for_status()

        response = await client.post(
            url,
            headers=headers,
            json={
                "jsonrpc": "2.0",
                "id": 2,
                "method": method,
                "params": params or {},
            },
        )
        message = _mcp_response_json(response)
        if "error" in message:
            raise RuntimeError(f"MCP error: {message['error']}")
        return message["result"]
```

Discover the current tool schemas first:

```python
exa_tools = await public_mcp_request(EXA_MCP_URL, "tools/list")
```

Then call a listed tool with arguments that match its `inputSchema`:

```python
exa_results = await public_mcp_request(
    EXA_MCP_URL,
    "tools/call",
    {
        "name": "web_search_exa",
        "arguments": {
            "query": "A precise natural-language description of the pages to find",
            "objective": "What should rank first and which facts to extract",
            "numResults": 10,
        },
    },
)

exa_pages = await public_mcp_request(
    EXA_MCP_URL,
    "tools/call",
    {
        "name": "web_fetch_exa",
        "arguments": {
            "urls": ["https://example.com/page"],
            "maxCharacters": 5000,
        },
    },
)
```

Do not interpret an empty Prime Env MCP connection list as an Exa failure. The
native `mcp` module lists Prime Env-managed connections; Exa is called directly.

## Exa authentication and limits

Anonymous Exa MCP calls are intended for casual use. The current server source
sets defaults of 2 `tools/call` requests per second and 50 per day, keyed by IP.
The hosted deployment can override those values. Protocol calls such as
`initialize` and `tools/list` do not consume this quota.

Use Exa tool calls sequentially when anonymous. Combine search goals where
possible, request several results per search, and batch multiple URLs in one
`web_fetch_exa` call. Stop on HTTP 429 rather than launching retries.

If the user has supplied an Exa API key through an approved secret source, pass
it as a header on every request in the session:

```python
request_headers={"x-api-key": exa_api_key}
```

Do not print or hardcode the key. Prefer `x-api-key` over putting it in a URL.

Official sources:

- [Exa MCP documentation](https://exa.ai/docs/reference/exa-mcp): anonymous
  access is intended for casual use; a key lifts the free MCP limit; 429 means
  the free limit was reached.
- [Exa MCP server source](https://github.com/exa-labs/exa-mcp-server/blob/master/api/mcp.ts):
  default rate limits and distinction between protocol calls and tool calls.
