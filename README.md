# cc-mcp-admin

Claude Code MCP Server Manager - A CLI tool to manage MCP servers across your projects.

## Features

- List all MCP servers across all projects with status indicators
- Show detailed configuration for specific servers with diff highlighting
- Add servers from other projects to current project with `name#N` config selection
- Remove servers from current project
- Migrate all projects from one config group to another
- Move per-project servers to global config
- Detect and group configuration differences by similarity
- Support global MCP servers (top-level `mcpServers` in `~/.claude.json`)

## Installation

```bash
cargo install --path .
```

Or build manually:

```bash
cargo build --release
cp .target/release/cc-mcp-admin ~/.local/bin/
```

## Usage

### List all MCP servers

```bash
cc-mcp-admin
# or
cc-mcp-admin list
```

Output shows:
- `●` (green) - enabled in current project
- `○` - not enabled in current project
- `(global)` - defined in top-level `mcpServers`
- `(N configs)` - N distinct configurations exist, grouped by `#1`, `#2`, etc.

### Show server details

```bash
cc-mcp-admin serena
# or
cc-mcp-admin show serena
```

Displays all configurations with differences highlighted in yellow.

### Add a server to current project

```bash
cc-mcp-admin add serena
```

If multiple configurations exist, use `#N` to specify which config group:

```bash
cc-mcp-admin add serena#1
cc-mcp-admin add vnc#2
```

You can also use `--from` with partial path matching:

```bash
cc-mcp-admin add serena --from slint
```

### Remove a server from current project

```bash
cc-mcp-admin remove serena
```

### Migrate configs across projects

Change all projects using config `#1` to config `#2`:

```bash
cc-mcp-admin migrate vnc 1 2
```

### Move a server to global config

```bash
cc-mcp-admin globalize codexreview
cc-mcp-admin globalize vnc#1    # specify config group
```

This moves the server to the top-level `mcpServers` in `~/.claude.json` and removes it from individual project configs.

## Configuration Sources

The tool reads MCP configurations from:

1. `~/.claude.json` - Top-level `mcpServers` (global, applies to all projects)
2. `~/.claude.json` - Per-project `mcpServers` under `projects`
3. `.mcp.json` - Project-local MCP configuration files

## License

MIT
