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
    /// Migrate all projects from one config to another (e.g. migrate vnc 1 2)
    Migrate {
        /// Name of the MCP server
        name: String,
        /// Source config group number
        from: usize,
        /// Target config group number
        to: usize,
    },
    /// Move a per-project MCP server to global config (e.g. globalize server-name)
    Globalize {
        /// Name of the MCP server (use name#N to specify config group)
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

/// Assign a config group index (1-based) to each entry based on normalized config similarity.
fn assign_config_groups(entries: &[McpEntry]) -> Vec<usize> {
    let mut groups = Vec::with_capacity(entries.len());
    let mut representatives: Vec<usize> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let norm_args = normalize_args(&entry.server.args, &entry.source_project);
        let found = representatives.iter().position(|&rep_idx| {
            let rep = &entries[rep_idx];
            let rep_args = normalize_args(&rep.server.args, &rep.source_project);
            entry.server.command == rep.server.command
                && entry.server.url == rep.server.url
                && norm_args == rep_args
                && entry.server.env == rep.server.env
        });
        match found {
            Some(pos) => groups.push(pos + 1),
            None => {
                representatives.push(i);
                groups.push(representatives.len());
            }
        }
    }
    groups
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
        let groups = assign_config_groups(entries);
        let n_groups = *groups.iter().max().unwrap_or(&1);
        let has_diff = n_groups > 1;

        let marker = if is_current {
            "●".green().to_string()
        } else {
            "○".dimmed().to_string()
        };

        let diff_marker = if has_diff {
            format!(" {}", format!("({} configs)", n_groups).yellow())
        } else {
            String::new()
        };

        let name_display = if is_current {
            name.green().bold().to_string()
        } else {
            name.to_string()
        };

        let is_global = entries.iter().any(|e| e.source_project == "(global)");
        let project_entries: Vec<_> = entries.iter().enumerate()
            .filter(|(_, e)| e.source_project != "(global)")
            .collect();

        let global_label = if is_global {
            format!(" {}", "(global)".dimmed())
        } else {
            String::new()
        };

        println!("  {} {}{}{}", marker, name_display, global_label, diff_marker);

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

        // Show projects using this server
        if !project_entries.is_empty() {
            if has_diff {
                for g in 1..=n_groups {
                    let group_projects: Vec<_> = project_entries.iter()
                        .filter(|(idx, _)| groups[*idx] == g)
                        .collect();
                    if group_projects.is_empty() {
                        continue;
                    }
                    println!("    {}",format!("#{}:", g).dimmed());
                    for (_, entry) in group_projects {
                        let is_cwd = entry.source_project == cwd;
                        let short_path = shorten_path(&entry.source_project);
                        if is_cwd {
                            println!("      {} {}", "→".green(), format!("{} (current)", short_path).green());
                        } else {
                            println!("      - {}", short_path);
                        }
                    }
                }
            } else {
                println!("    {}", "used in:".dimmed());
                for (_, entry) in &project_entries {
                    let is_cwd = entry.source_project == cwd;
                    let short_path = shorten_path(&entry.source_project);
                    if is_cwd {
                        println!("      {} {}", "→".green(), format!("{} (current)", short_path).green());
                    } else {
                        println!("      - {}", short_path);
                    }
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

            let is_global = entries.iter().any(|e| e.source_project == "(global)");
            let scope = if is_global { "global" } else { "per-project" };
            println!("{} {} {}", "MCP Server:".bold(), name.bold(), format!("({})", scope).dimmed());
            println!("  {} {}", "Status:".dimmed(), status);
            println!();

            let groups = assign_config_groups(entries);
            let n_groups = *groups.iter().max().unwrap_or(&1);
            let project_entries: Vec<_> = entries.iter().enumerate()
                .filter(|(_, e)| e.source_project != "(global)")
                .collect();

            let baseline = &entries[0];
            let baseline_args = normalize_args(&baseline.server.args, &baseline.source_project);

            for g in 1..=n_groups {
                // Find the representative entry for this group
                let rep_idx = groups.iter().position(|&gi| gi == g).unwrap();
                let rep = &entries[rep_idx];

                if n_groups > 1 {
                    println!("  {}", format!("#{}:", g).bold());
                }

                // Show command/url, highlight if different from baseline
                if let Some(ref cmd) = rep.server.command {
                    let cmd_display = if g > 1 && rep.server.command != baseline.server.command {
                        cmd.yellow().to_string()
                    } else {
                        cmd.clone()
                    };
                    println!("    {} {}", "command:".dimmed(), cmd_display);
                } else if let Some(ref url) = rep.server.url {
                    let url_display = if g > 1 && rep.server.url != baseline.server.url {
                        url.yellow().to_string()
                    } else {
                        url.clone()
                    };
                    println!("    {} {}", "url:".dimmed(), url_display);
                }

                // Show args, highlight differences from baseline
                if !rep.server.args.is_empty() {
                    let normalized = normalize_args(&rep.server.args, &rep.source_project);
                    if g > 1 && normalized != baseline_args {
                        let highlighted: Vec<String> = normalized
                            .iter()
                            .enumerate()
                            .map(|(j, norm)| {
                                let baseline_norm = baseline_args.get(j);
                                if baseline_norm != Some(norm) {
                                    format!("\"{}\"", norm).yellow().to_string()
                                } else {
                                    format!("\"{}\"", norm)
                                }
                            })
                            .collect();
                        println!("    {} [{}]", "args:".dimmed(), highlighted.join(", "));
                    } else {
                        let display: Vec<String> = normalize_args(&rep.server.args, &rep.source_project)
                            .iter()
                            .map(|a| format!("\"{}\"", a))
                            .collect();
                        println!("    {} [{}]", "args:".dimmed(), display.join(", "));
                    }
                }

                // Show env, highlight differences
                if !rep.server.env.is_empty() {
                    if g > 1 && rep.server.env != baseline.server.env {
                        println!("    {} {}", "env:".dimmed(), format!("{:?}", rep.server.env).yellow());
                    } else {
                        println!("    {} {:?}", "env:".dimmed(), rep.server.env);
                    }
                } else if g > 1 && !baseline.server.env.is_empty() {
                    println!("    {} {}", "env:".dimmed(), "(none)".yellow());
                }

                // Show projects in this group (skip global entries)
                let group_projects: Vec<_> = project_entries.iter()
                    .filter(|(idx, _)| groups[*idx] == g)
                    .collect();
                if !group_projects.is_empty() {
                    println!("    {}", "used in:".dimmed());
                    for (_, entry) in group_projects {
                        println!("      - {}", shorten_path(&entry.source_project));
                    }
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

/// Parse `name#N` syntax. Returns (server_name, Option<1-based index>).
fn parse_name_index(input: &str) -> (&str, Option<usize>) {
    if let Some(pos) = input.rfind('#') {
        if let Ok(idx) = input[pos + 1..].parse::<usize>() {
            return (&input[..pos], Some(idx));
        }
    }
    (input, None)
}

fn add_mcp_server(name: &str, from: Option<&str>) {
    let (server_name, config_index) = parse_name_index(name);

    let all_servers = collect_all_mcp_servers();
    let current_servers = get_current_project_mcp_servers();
    let cwd = env::current_dir().expect("Failed to get current directory");
    let cwd_str = cwd.to_string_lossy().to_string();

    if current_servers.contains_key(server_name) {
        println!(
            "{} MCP server '{}' is already enabled in this project",
            "Note:".yellow(),
            server_name
        );
        return;
    }

    let entries = match all_servers.get(server_name) {
        Some(e) => e,
        None => {
            eprintln!(
                "{} MCP server '{}' not found in any project",
                "Error:".red(),
                server_name
            );
            std::process::exit(1);
        }
    };

    let groups = assign_config_groups(entries);
    let n_groups = *groups.iter().max().unwrap_or(&1);
    let has_diff = n_groups > 1;

    // Select configuration based on #N group, --from option, or show options if multiple exist
    let entry = if let Some(idx) = config_index {
        if idx == 0 || idx > n_groups {
            eprintln!(
                "{} Invalid config index #{}. Available: #1-#{}",
                "Error:".red(),
                idx,
                n_groups
            );
            eprintln!("Use 'cc-mcp-admin show {}' for details.", server_name);
            std::process::exit(1);
        }
        // Pick the first entry belonging to the requested group
        let pos = groups.iter().position(|&g| g == idx).unwrap();
        &entries[pos]
    } else if let Some(from_pattern) = from {
        match entries.iter().find(|e| e.source_project.contains(from_pattern)) {
            Some(e) => e,
            None => {
                eprintln!(
                    "{} No configuration found matching '{}'",
                    "Error:".red(),
                    from_pattern
                );
                eprintln!("Use 'cc-mcp-admin show {}' for details.", server_name);
                std::process::exit(1);
            }
        }
    } else if has_diff {
        eprintln!(
            "{} {} configurations found for '{}'. Use #N to specify:",
            "Error:".red(),
            n_groups,
            server_name
        );
        for g in 1..=n_groups {
            let rep_idx = groups.iter().position(|&gi| gi == g).unwrap();
            let rep = &entries[rep_idx];
            let target = rep.server.display_target();
            let count = groups.iter().filter(|&&gi| gi == g).count();
            eprintln!(
                "  #{} {} ({} {})",
                g,
                target,
                count,
                if count == 1 { "project" } else { "projects" }
            );
        }
        eprintln!();
        eprintln!("Example: cc-mcp-admin add {}#1", server_name);
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
    json["projects"][&cwd_str]["mcpServers"][server_name] = serde_json::to_value(&server).unwrap();

    // Write back
    let new_content = serde_json::to_string_pretty(&json).expect("Failed to serialize JSON");
    fs::write(&claude_json_path, new_content).expect("Failed to write ~/.claude.json");

    println!(
        "{} Added MCP server '{}' to current project",
        "✓".green(),
        server_name.green().bold()
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

fn migrate_mcp_server(name: &str, from: usize, to: usize) {
    let all_servers = collect_all_mcp_servers();

    let entries = match all_servers.get(name) {
        Some(e) => e,
        None => {
            eprintln!("{} MCP server '{}' not found", "Error:".red(), name);
            std::process::exit(1);
        }
    };

    let groups = assign_config_groups(entries);
    let n_groups = *groups.iter().max().unwrap_or(&1);

    if from == 0 || from > n_groups || to == 0 || to > n_groups {
        eprintln!(
            "{} Invalid config group. Available: #1-#{}",
            "Error:".red(),
            n_groups
        );
        std::process::exit(1);
    }

    if from == to {
        println!("{} #{}  and #{} are the same group", "Note:".yellow(), from, to);
        return;
    }

    // Collect projects to migrate (only ~/.claude.json per-project entries)
    let target_projects: Vec<&str> = entries
        .iter()
        .enumerate()
        .filter(|(idx, e)| groups[*idx] == from && e.source_project != "(global)")
        .map(|(_, e)| e.source_project.as_str())
        .collect();

    // Check for .mcp.json entries that can't be auto-migrated
    let mcp_json_projects: Vec<&str> = entries
        .iter()
        .enumerate()
        .filter(|(idx, _)| groups[*idx] == from)
        .filter(|(_, e)| {
            let path = std::path::Path::new(&e.source_project).join(".mcp.json");
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(mcp_json) = serde_json::from_str::<McpJsonFile>(&content) {
                        return mcp_json.mcp_servers.contains_key(name);
                    }
                }
            }
            false
        })
        .map(|(_, e)| e.source_project.as_str())
        .collect();

    if !mcp_json_projects.is_empty() {
        println!(
            "{} The following projects define '{}' in .mcp.json (manual update needed):",
            "Note:".yellow(),
            name
        );
        for p in &mcp_json_projects {
            println!("  - {}", shorten_path(p));
        }
        println!();
    }

    // Filter to only ~/.claude.json per-project entries (exclude .mcp.json-only ones)
    let claude_json_path = get_claude_json_path().expect("Failed to get claude.json path");
    let content = fs::read_to_string(&claude_json_path).expect("Failed to read ~/.claude.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&content).expect("Failed to parse ~/.claude.json");

    // Get the representative config from the target group
    let to_rep_idx = groups.iter().position(|&g| g == to).unwrap();
    let to_rep = &entries[to_rep_idx];

    let mut migrated = 0;
    for project_path in &target_projects {
        // Check this project has the server in ~/.claude.json (not only in .mcp.json)
        let has_in_claude_json = json
            .get("projects")
            .and_then(|p| p.get(*project_path))
            .and_then(|c| c.get("mcpServers"))
            .and_then(|m| m.get(name))
            .is_some();

        if !has_in_claude_json {
            continue;
        }

        // Build new config with project-specific path substitution
        let mut new_server = to_rep.server.clone();
        for arg in &mut new_server.args {
            if arg.contains(&to_rep.source_project) {
                *arg = arg.replace(&to_rep.source_project, project_path);
            }
        }

        json["projects"][*project_path]["mcpServers"][name] =
            serde_json::to_value(&new_server).unwrap();
        migrated += 1;
    }

    if migrated > 0 {
        let new_content = serde_json::to_string_pretty(&json).expect("Failed to serialize JSON");
        fs::write(&claude_json_path, new_content).expect("Failed to write ~/.claude.json");
    }

    println!(
        "{} Migrated {} {} from #{} to #{} for '{}'",
        "✓".green(),
        migrated,
        if migrated == 1 { "project" } else { "projects" },
        from,
        to,
        name.green().bold()
    );
}

fn globalize_mcp_server(name: &str) {
    let (server_name, config_index) = parse_name_index(name);
    let all_servers = collect_all_mcp_servers();

    let entries = match all_servers.get(server_name) {
        Some(e) => e,
        None => {
            eprintln!("{} MCP server '{}' not found", "Error:".red(), server_name);
            std::process::exit(1);
        }
    };

    // Check if already global
    if entries.iter().any(|e| e.source_project == "(global)") {
        println!(
            "{} MCP server '{}' is already defined globally",
            "Note:".yellow(),
            server_name
        );
        return;
    }

    let groups = assign_config_groups(entries);
    let n_groups = *groups.iter().max().unwrap_or(&1);

    // Select which config group to globalize
    let group = if let Some(idx) = config_index {
        if idx == 0 || idx > n_groups {
            eprintln!(
                "{} Invalid config index #{}. Available: #1-#{}",
                "Error:".red(),
                idx,
                n_groups
            );
            std::process::exit(1);
        }
        idx
    } else if n_groups > 1 {
        eprintln!(
            "{} {} configurations found for '{}'. Use #N to specify:",
            "Error:".red(),
            n_groups,
            server_name
        );
        for g in 1..=n_groups {
            let rep_idx = groups.iter().position(|&gi| gi == g).unwrap();
            let rep = &entries[rep_idx];
            let target = rep.server.display_target();
            let count = groups.iter().filter(|&&gi| gi == g).count();
            eprintln!(
                "  #{} {} ({} {})",
                g,
                target,
                count,
                if count == 1 { "project" } else { "projects" }
            );
        }
        eprintln!();
        eprintln!("Example: cc-mcp-admin globalize {}#1", server_name);
        std::process::exit(1);
    } else {
        1
    };

    // Get representative config (with project paths removed from args)
    let rep_idx = groups.iter().position(|&g| g == group).unwrap();
    let rep = &entries[rep_idx];
    let mut global_server = rep.server.clone();
    // Strip project-specific paths from args for global config
    for arg in &mut global_server.args {
        if arg.contains(&rep.source_project) {
            *arg = arg.replace(&rep.source_project, "<PROJECT>");
        }
    }

    // Warn if args contain project-specific placeholder
    let has_project_path = global_server.args.iter().any(|a| a.contains("<PROJECT>"));
    if has_project_path {
        eprintln!(
            "{} This server's args contain project-specific paths.",
            "Warning:".yellow()
        );
        eprintln!("  Global config will use '<PROJECT>' as placeholder, which may not work.");
        eprintln!("  Consider whether this server is suitable for global use.");
        eprintln!();
    }

    let claude_json_path = get_claude_json_path().expect("Failed to get claude.json path");
    let content = fs::read_to_string(&claude_json_path).expect("Failed to read ~/.claude.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&content).expect("Failed to parse ~/.claude.json");

    // Add to top-level mcpServers
    if json.get("mcpServers").is_none() {
        json["mcpServers"] = serde_json::json!({});
    }
    json["mcpServers"][server_name] = serde_json::to_value(&global_server).unwrap();

    // Remove from all per-project configs in the same group
    let mut removed = 0;
    let mut mcp_json_warn = Vec::new();
    for (idx, entry) in entries.iter().enumerate() {
        if groups[idx] != group {
            continue;
        }

        // Check if also in .mcp.json
        let mcp_path = std::path::Path::new(&entry.source_project).join(".mcp.json");
        if mcp_path.exists() {
            if let Ok(c) = fs::read_to_string(&mcp_path) {
                if let Ok(mj) = serde_json::from_str::<McpJsonFile>(&c) {
                    if mj.mcp_servers.contains_key(server_name) {
                        mcp_json_warn.push(entry.source_project.clone());
                    }
                }
            }
        }

        // Remove from ~/.claude.json per-project
        if let Some(mcp_servers) = json
            .get_mut("projects")
            .and_then(|p| p.get_mut(&entry.source_project))
            .and_then(|c| c.get_mut("mcpServers"))
            .and_then(|m| m.as_object_mut())
        {
            if mcp_servers.remove(server_name).is_some() {
                removed += 1;
            }
        }
    }

    let new_content = serde_json::to_string_pretty(&json).expect("Failed to serialize JSON");
    fs::write(&claude_json_path, new_content).expect("Failed to write ~/.claude.json");

    println!(
        "{} Moved '{}' to global config (removed from {} {})",
        "✓".green(),
        server_name.green().bold(),
        removed,
        if removed == 1 { "project" } else { "projects" }
    );

    if !mcp_json_warn.is_empty() {
        println!();
        println!(
            "{} Also defined in .mcp.json (remove manually):",
            "Note:".yellow()
        );
        for p in &mcp_json_warn {
            println!("  - {}", shorten_path(p));
        }
    }
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
        Some(Commands::Migrate { name, from, to }) => migrate_mcp_server(&name, from, to),
        Some(Commands::Globalize { name }) => globalize_mcp_server(&name),
    }
}
