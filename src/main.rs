use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::{env, fs};

#[derive(Parser)]
#[command(name = "cc-mcp-admin")]
#[command(about = "Claude Code MCP Server Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// MCP server name to show (shorthand for 'show <name>')
    name: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// List all MCP servers across all projects
    List,
    /// Add an MCP server to the current project
    Add {
        /// Name of the MCP server to add
        name: String,
        /// Source project to copy configuration from (use partial path match)
        #[arg(long)]
        from: Option<String>,
    },
    /// Remove an MCP server from the current project
    Remove {
        /// Name of the MCP server to remove
        name: String,
    },
    /// Show details of a specific MCP server
    Show {
        /// Name of the MCP server
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpServer {
    #[serde(rename = "type")]
    server_type: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

impl McpServer {
    fn display_target(&self) -> &str {
        if let Some(ref cmd) = self.command {
            cmd
        } else if let Some(ref url) = self.url {
            url
        } else {
            "(unknown)"
        }
    }
}

#[derive(Debug, Deserialize)]
struct McpJsonFile {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, McpServer>,
}

#[derive(Debug, Deserialize)]
struct ProjectConfig {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, McpServer>,
}

#[derive(Debug, Deserialize)]
struct ClaudeJson {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, McpServer>,
    #[serde(default)]
    projects: HashMap<String, ProjectConfig>,
}

#[derive(Debug, Clone)]
struct McpEntry {
    server: McpServer,
    source_project: String,
}

fn get_claude_json_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude.json"))
}

fn load_claude_json() -> Option<ClaudeJson> {
    let path = get_claude_json_path()?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn find_mcp_json_files() -> Vec<PathBuf> {
    let mut mcp_files = Vec::new();

    // Get project paths from ~/.claude.json and check for .mcp.json in each
    if let Some(claude_json) = load_claude_json() {
        for project_path in claude_json.projects.keys() {
            let mcp_path = PathBuf::from(project_path).join(".mcp.json");
            if mcp_path.exists() {
                mcp_files.push(mcp_path);
            }
        }
    }

    mcp_files
}

fn collect_all_mcp_servers() -> HashMap<String, Vec<McpEntry>> {
    let mut all_servers: HashMap<String, Vec<McpEntry>> = HashMap::new();

    // Load from ~/.claude.json
    if let Some(claude_json) = load_claude_json() {
        // Global mcpServers (top-level)
        for (name, server) in claude_json.mcp_servers {
            let entry = McpEntry {
                server,
                source_project: "(global)".to_string(),
            };
            all_servers.entry(name).or_default().push(entry);
        }

        // Per-project mcpServers
        for (project_path, config) in claude_json.projects {
            for (name, server) in config.mcp_servers {
                let entry = McpEntry {
                    server,
                    source_project: project_path.clone(),
                };
                all_servers.entry(name).or_default().push(entry);
            }
        }
    }

    // Load from .mcp.json files
    for mcp_path in find_mcp_json_files() {
        if let Ok(content) = fs::read_to_string(&mcp_path) {
            if let Ok(mcp_json) = serde_json::from_str::<McpJsonFile>(&content) {
                let project_path = mcp_path
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();

                for (name, server) in mcp_json.mcp_servers {
                    let entry = McpEntry {
                        server,
                        source_project: project_path.clone(),
                    };
                    all_servers.entry(name).or_default().push(entry);
                }
            }
        }
    }

    // Sort entries by source_project for deterministic order
    for entries in all_servers.values_mut() {
        entries.sort_by(|a, b| a.source_project.cmp(&b.source_project));
    }

    all_servers
}

fn get_current_project_mcp_servers() -> HashMap<String, McpServer> {
    let cwd = env::current_dir().ok();
    let cwd_str = cwd.as_ref().map(|p| p.to_string_lossy().to_string());

    let mut servers = HashMap::new();

    // Check ~/.claude.json
    if let Some(claude_json) = load_claude_json() {
        // Global mcpServers apply to all projects
        servers.extend(claude_json.mcp_servers.clone());

        // Per-project mcpServers for current directory
        if let Some(cwd) = &cwd_str {
            if let Some(config) = claude_json.projects.get(cwd) {
                servers.extend(config.mcp_servers.clone());
            }
        }
    }

    // Check local .mcp.json
    if let Some(ref cwd_path) = cwd {
        let mcp_json_path = cwd_path.join(".mcp.json");
        if let Ok(content) = fs::read_to_string(&mcp_json_path) {
            if let Ok(mcp_json) = serde_json::from_str::<McpJsonFile>(&content) {
                servers.extend(mcp_json.mcp_servers);
            }
        }
    }

    servers
}

/// Normalize args by replacing project-specific paths with a placeholder
fn normalize_args(args: &[String], project_path: &str) -> Vec<String> {
    args.iter()
        .map(|arg| arg.replace(project_path, "<PROJECT>"))
        .collect()
}

fn configs_differ(entries: &[McpEntry]) -> bool {
    if entries.len() <= 1 {
        return false;
    }
    let first = &entries[0];
    let first_args = normalize_args(&first.server.args, &first.source_project);

    entries.iter().skip(1).any(|e| {
        let args = normalize_args(&e.server.args, &e.source_project);
        e.server.command != first.server.command
            || e.server.url != first.server.url
            || args != first_args
            || e.server.env != first.server.env
    })
}

fn list_mcp_servers() {
    let all_servers = collect_all_mcp_servers();
    let current_servers = get_current_project_mcp_servers();
    let cwd = env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    if all_servers.is_empty() {
        println!("No MCP servers found across any projects.");
        return;
    }

    println!("{}", "MCP Servers:".bold());
    println!();

    let mut names: Vec<_> = all_servers.keys().collect();
    names.sort();

    for name in names {
        let entries = &all_servers[name];
        let is_current = current_servers.contains_key(name);
        let has_diff = configs_differ(entries);

        let marker = if is_current {
            "●".green().to_string()
        } else {
            "○".dimmed().to_string()
        };

        let diff_marker = if has_diff {
            format!(" {}", "(multiple configs)".yellow())
        } else {
            String::new()
        };

        let name_display = if is_current {
            name.green().bold().to_string()
        } else {
            name.to_string()
        };

        println!("  {} {}{}", marker, name_display, diff_marker);

        // Show command/url (note if configs differ)
        if let Some(entry) = entries.first() {
            let target = entry.server.display_target();
            let label = if entry.server.url.is_some() { "url:" } else { "command:" };
            if has_diff {
                println!("    {} {} {}", label.dimmed(), target, "(varies)".dimmed());
            } else {
                println!("    {} {}", label.dimmed(), target);
            }
        }

        // Show projects using this server (sorted)
        println!("    {}", "used in:".dimmed());
        let mut sorted_entries: Vec<_> = entries.iter().collect();
        sorted_entries.sort_by_key(|e| &e.source_project);
        for entry in sorted_entries {
            if entry.source_project == "(global)" {
                println!("      - {}", "(global)".dimmed());
            } else {
                let is_cwd = entry.source_project == cwd;
                let short_path = shorten_path(&entry.source_project);
                if is_cwd {
                    println!("      {} {}", "→".green(), format!("{} (current)", short_path).green());
                } else {
                    println!("      - {}", short_path);
                }
            }
        }
        println!();
    }

    println!(
        "{}",
        format!(
            "Total: {} unique MCP servers across all projects",
            all_servers.len()
        )
        .dimmed()
    );
    println!(
        "{}",
        format!("Current project: {} servers enabled", current_servers.len()).dimmed()
    );
}

fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path.starts_with(home_str.as_ref()) {
            return path.replacen(home_str.as_ref(), "~", 1);
        }
    }
    path.to_string()
}

fn show_mcp_server(name: &str) {
    let all_servers = collect_all_mcp_servers();
    let current_servers = get_current_project_mcp_servers();

    match all_servers.get(name) {
        Some(entries) => {
            let is_current = current_servers.contains_key(name);
            let status = if is_current {
                "enabled in current project".green()
            } else {
                "not enabled in current project".yellow()
            };

            println!("{} {}", "MCP Server:".bold(), name.bold());
            println!("  {} {}", "Status:".dimmed(), status);
            println!();

            // Use first entry as baseline for comparison
            let baseline = &entries[0];
            let baseline_args = normalize_args(&baseline.server.args, &baseline.source_project);

            for (i, entry) in entries.iter().enumerate() {
                let source_display = if entry.source_project == "(global)" {
                    "(global)".to_string()
                } else {
                    shorten_path(&entry.source_project)
                };
                println!(
                    "  {} {}",
                    format!("Configuration #{}:", i + 1).bold(),
                    source_display.dimmed()
                );

                // Highlight command/url if different from baseline
                if let Some(ref cmd) = entry.server.command {
                    let cmd_display = if i > 0 && entry.server.command != baseline.server.command {
                        cmd.yellow().to_string()
                    } else {
                        cmd.clone()
                    };
                    println!("    {} {}", "command:".dimmed(), cmd_display);
                } else if let Some(ref url) = entry.server.url {
                    let url_display = if i > 0 && entry.server.url != baseline.server.url {
                        url.yellow().to_string()
                    } else {
                        url.clone()
                    };
                    println!("    {} {}", "url:".dimmed(), url_display);
                }

                // Highlight args differences
                if !entry.server.args.is_empty() {
                    let normalized = normalize_args(&entry.server.args, &entry.source_project);
                    if i > 0 && normalized != baseline_args {
                        // Show args with differences highlighted
                        let highlighted: Vec<String> = entry
                            .server
                            .args
                            .iter()
                            .zip(normalized.iter())
                            .enumerate()
                            .map(|(j, (orig, norm))| {
                                let baseline_norm = baseline_args.get(j);
                                if baseline_norm != Some(norm) {
                                    format!("\"{}\"", orig).yellow().to_string()
                                } else {
                                    format!("\"{}\"", orig)
                                }
                            })
                            .collect();
                        println!("    {} [{}]", "args:".dimmed(), highlighted.join(", "));
                    } else {
                        println!("    {} {:?}", "args:".dimmed(), entry.server.args);
                    }
                }

                // Highlight env if different from baseline
                if !entry.server.env.is_empty() {
                    if i > 0 && entry.server.env != baseline.server.env {
                        println!("    {} {}", "env:".dimmed(), format!("{:?}", entry.server.env).yellow());
                    } else {
                        println!("    {} {:?}", "env:".dimmed(), entry.server.env);
                    }
                } else if i > 0 && !baseline.server.env.is_empty() {
                    // Baseline has env but this one doesn't
                    println!("    {} {}", "env:".dimmed(), "(none)".yellow());
                }
                println!();
            }
        }
        None => {
            eprintln!("{} MCP server '{}' not found", "Error:".red(), name);
            std::process::exit(1);
        }
    }
}

fn add_mcp_server(name: &str, from: Option<&str>) {
    let all_servers = collect_all_mcp_servers();
    let current_servers = get_current_project_mcp_servers();
    let cwd = env::current_dir().expect("Failed to get current directory");
    let cwd_str = cwd.to_string_lossy().to_string();

    if current_servers.contains_key(name) {
        println!(
            "{} MCP server '{}' is already enabled in this project",
            "Note:".yellow(),
            name
        );
        return;
    }

    let entries = match all_servers.get(name) {
        Some(e) => e,
        None => {
            eprintln!(
                "{} MCP server '{}' not found in any project",
                "Error:".red(),
                name
            );
            std::process::exit(1);
        }
    };

    // Select configuration based on --from option or show options if multiple exist
    let entry = if let Some(from_pattern) = from {
        match entries.iter().find(|e| e.source_project.contains(from_pattern)) {
            Some(e) => e,
            None => {
                eprintln!(
                    "{} No configuration found matching '{}'",
                    "Error:".red(),
                    from_pattern
                );
                eprintln!("Available configurations:");
                for e in entries {
                    eprintln!("  - {}", shorten_path(&e.source_project));
                }
                std::process::exit(1);
            }
        }
    } else if entries.len() > 1 && configs_differ(entries) {
        eprintln!(
            "{} Multiple configurations found for '{}'. Use --from to specify:",
            "Error:".red(),
            name
        );
        for e in entries {
            eprintln!("  {} {}", "→".dimmed(), shorten_path(&e.source_project));
        }
        eprintln!();
        eprintln!("Example: cc-mcp-admin add {} --from votingmachine", name);
        std::process::exit(1);
    } else {
        &entries[0]
    };

    let mut server = entry.server.clone();

    // Update project path in args if this is a serena-style server
    for arg in &mut server.args {
        if arg.contains(&entry.source_project) {
            *arg = arg.replace(&entry.source_project, &cwd_str);
        }
    }

    // Update to ~/.claude.json
    let claude_json_path = get_claude_json_path().expect("Failed to get claude.json path");
    let content = fs::read_to_string(&claude_json_path).expect("Failed to read ~/.claude.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&content).expect("Failed to parse ~/.claude.json");

    // Ensure projects object exists
    if json.get("projects").is_none() {
        json["projects"] = serde_json::json!({});
    }

    // Ensure current project exists
    if json["projects"].get(&cwd_str).is_none() {
        json["projects"][&cwd_str] = serde_json::json!({
            "mcpServers": {}
        });
    }

    // Ensure mcpServers exists
    if json["projects"][&cwd_str].get("mcpServers").is_none() {
        json["projects"][&cwd_str]["mcpServers"] = serde_json::json!({});
    }

    // Add the server
    json["projects"][&cwd_str]["mcpServers"][name] = serde_json::to_value(&server).unwrap();

    // Write back
    let new_content = serde_json::to_string_pretty(&json).expect("Failed to serialize JSON");
    fs::write(&claude_json_path, new_content).expect("Failed to write ~/.claude.json");

    println!(
        "{} Added MCP server '{}' to current project",
        "✓".green(),
        name.green().bold()
    );
    if let Some(ref cmd) = server.command {
        println!("  {} {}", "command:".dimmed(), cmd);
    } else if let Some(ref url) = server.url {
        println!("  {} {}", "url:".dimmed(), url);
    }
    if !server.args.is_empty() {
        println!("  {} {:?}", "args:".dimmed(), server.args);
    }
}

fn remove_mcp_server(name: &str) {
    let current_servers = get_current_project_mcp_servers();
    let cwd = env::current_dir().expect("Failed to get current directory");
    let cwd_str = cwd.to_string_lossy().to_string();

    if !current_servers.contains_key(name) {
        eprintln!(
            "{} MCP server '{}' is not enabled in this project",
            "Error:".red(),
            name
        );
        std::process::exit(1);
    }

    // Check if it's a global server
    if let Some(claude_json) = load_claude_json() {
        if claude_json.mcp_servers.contains_key(name) {
            println!(
                "{} MCP server '{}' is defined globally in ~/.claude.json (top-level mcpServers)",
                "Note:".yellow(),
                name
            );
            println!("  Please remove it manually from ~/.claude.json");
            return;
        }
    }

    // Check if it's in local .mcp.json
    let mcp_json_path = cwd.join(".mcp.json");
    if mcp_json_path.exists() {
        if let Ok(content) = fs::read_to_string(&mcp_json_path) {
            if let Ok(mcp_json) = serde_json::from_str::<McpJsonFile>(&content) {
                if mcp_json.mcp_servers.contains_key(name) {
                    println!(
                        "{} MCP server '{}' is defined in local .mcp.json",
                        "Note:".yellow(),
                        name
                    );
                    println!("  Please remove it manually from .mcp.json");
                    return;
                }
            }
        }
    }

    // Remove from ~/.claude.json
    let claude_json_path = get_claude_json_path().expect("Failed to get claude.json path");
    let content = fs::read_to_string(&claude_json_path).expect("Failed to read ~/.claude.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&content).expect("Failed to parse ~/.claude.json");

    if let Some(mcp_servers) = json
        .get_mut("projects")
        .and_then(|p| p.get_mut(&cwd_str))
        .and_then(|c| c.get_mut("mcpServers"))
        .and_then(|m| m.as_object_mut())
    {
        mcp_servers.remove(name);
    }

    let new_content = serde_json::to_string_pretty(&json).expect("Failed to serialize JSON");
    fs::write(&claude_json_path, new_content).expect("Failed to write ~/.claude.json");

    println!(
        "{} Removed MCP server '{}' from current project",
        "✓".green(),
        name.green().bold()
    );
}

fn main() {
    let cli = Cli::parse();

    // Handle shorthand: cc-mcp-admin <name> => cc-mcp-admin show <name>
    if let Some(name) = cli.name {
        show_mcp_server(&name);
        return;
    }

    match cli.command {
        Some(Commands::List) | None => list_mcp_servers(),
        Some(Commands::Add { name, from }) => add_mcp_server(&name, from.as_deref()),
        Some(Commands::Remove { name }) => remove_mcp_server(&name),
        Some(Commands::Show { name }) => show_mcp_server(&name),
    }
}
