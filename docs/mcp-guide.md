# Adding MCP servers

harnessUI can use tools from any [Model Context Protocol](https://modelcontextprotocol.io)
server, alongside its built-in tools (files, terminal, git, browser, computer use,
skills). Every tool, built-in or added, goes through the same approval cards, per-chat
toggles and audit log.

## Before you add one

An MCP server you add **runs with your own permissions**. It can read and change anything
you can. Only add servers you trust, from sources you trust. The app asks you to confirm
this when you add a server, and records that you did.

## Adding a server

Open **Settings → MCP servers → Add servers** and paste a configuration. The app accepts the
formats other apps use, so you can usually copy a server's README example as it is.

Claude Desktop and Cursor format:

```json
{
  "mcpServers": {
    "filesystem-docs": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/Documents"]
    }
  }
}
```

VS Code format:

```json
{
  "servers": {
    "weather": {
      "type": "stdio",
      "command": "uvx",
      "args": ["weather-mcp"],
      "env": { "WEATHER_API_KEY": "..." }
    }
  }
}
```

A remote server over Streamable HTTP:

```json
{
  "mcpServers": {
    "tickets": {
      "type": "http",
      "url": "https://mcp.example.com/mcp",
      "headers": { "Authorization": "Bearer ..." }
    }
  }
}
```

### Secrets

Values in `env` and `headers` are moved to your system keychain when you save; the
database keeps only a reference, and an exported configuration leaves them out. If a
token appears in `args` or in a URL (for example `--api-key abc123` or
`https://user:password@host`), the app offers to move it to the keychain and leaves a
placeholder such as `${secret:tickets-token}` in its place.

### What the server receives

A local server starts with a short list of environment variables (PATH, your home and
temporary folders, and the few Windows needs) plus the `env` from its configuration. It
does not inherit everything set in your shell, so keys you keep there are not handed to
it.

## Using its tools

- A newly added server's tools start **off** in every chat. Turn them on in a chat's tool
  list (the toggle next to each tool, or the whole server at once).
- If a server changes a tool's name, description or schema, or adds a new tool later, that
  tool starts off again and loses any grants, so a server cannot quietly change what you
  approved.
- Each call shows an approval card with the server, the tool and the full arguments.
  *Allow for this chat* covers that tool with exactly those arguments. If you tick
  *allow any arguments* for a tool, a grant covers every call to it.
- Tools the server marks as destructive always ask. You can mark any tool *always ask*
  yourself. A server saying a tool is read-only never skips a prompt.
- **Stop** cancels a running call. For a local server the app sends the cancel and, if the
  server does not stop, ends its process half a second later and restarts it the next
  time it is needed.

## Troubleshooting

Each server shows **Starting**, **Ready**, **Failed** (with the reason) or **Stopped**,
and the last lines of its error output next to any start-up error.

| What you see | What to do |
|---|---|
| "This server needs Node.js, which isn't installed" | Install Node.js from nodejs.org, then press Restart. The same message names uv/Python or Docker when those are missing. |
| It stays on Starting for a while the first time | `npx -y` and `uvx` download the server on first use; the app waits up to 60 seconds. |
| "uses the legacy SSE transport, which is not supported" | The server only speaks the old HTTP+SSE transport. Run it through the `mcp-remote` bridge as a local server instead. |
| macOS asks to install developer tools | The app never runs Apple's `python3` or `git` stubs; install the Command Line Tools (`xcode-select --install`) or a real Python. |
| A tool is greyed out with a schema error | The server's input schema uses something the app refuses for safety (for example a `$ref` to another file or URL). Ask the server's author to inline it. |

## What is not supported in version 1

MCP resources and prompts, skills served over MCP, `.mcpb` bundles, and the legacy
HTTP+SSE transport. Servers that ask the app to sample from the model are refused; a
request for roots gets an empty list; a request for user input is declined.
