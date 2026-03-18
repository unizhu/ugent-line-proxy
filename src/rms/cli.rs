//! RMS CLI
//!
//! Command-line interface for the Relationship Management System.

use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table, presets::UTF8_FULL};
use std::io::{self, Read};

use super::service::RelationshipManagerService;
use super::types::{EntityFilter, LineEntityType, RelationshipImport, SystemStatus, ClientInfo, LineEntity, Relationship, DispatchRule};

/// RMS CLI - Relationship Management System
#[derive(Parser, Debug)]
#[command(name = "rms-cli")]
#[command(about = "Manage LINE-to-UGENT relationships", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show system status
    Status,

    /// Manage clients
    Clients {
        #[command(subcommand)]
        cmd: ClientCommands,
    },

    /// Manage LINE entities
    Entities {
        #[command(subcommand)]
        cmd: EntityCommands,
    },

    /// Manage relationships
    Relationships {
        #[command(subcommand)]
        cmd: RelationshipCommands,
    },

    /// View dispatch rules
    Rules {
        #[command(subcommand)]
        cmd: RuleCommands,
    },

    /// Import/export relationships
    Import,
    Export,

    /// Sync with runtime ownership
    Sync,
}

#[derive(Subcommand, Debug)]
enum ClientCommands {
    /// List all clients
    List,
    /// Show client details
    Show { id: String },
}

#[derive(Subcommand, Debug)]
enum EntityCommands {
    /// List all entities
    List {
        /// Filter by type (user, group, room)
        #[arg(short, long)]
        r#type: Option<String>,
        /// Only show entities with relationships
        #[arg(short, long)]
        assigned: bool,
        /// Only show entities without relationships
        #[arg(long)]
        unassigned: bool,
        /// Search by display name
        #[arg(short, long)]
        search: Option<String>,
    },
    /// Show entity details
    Show { id: String },
    /// Refresh entity from LINE API
    Refresh { id: String },
}

#[derive(Subcommand, Debug)]
enum RelationshipCommands {
    /// List all relationships
    List,
    /// Show relationship for an entity
    Show { entity_id: String },
    /// Set a relationship (create or update)
    Set {
        /// LINE entity ID
        entity_id: String,
        /// Client ID to assign
        client_id: String,
        /// Admin notes
        #[arg(short, long)]
        notes: Option<String>,
    },
    /// Remove a relationship
    Remove { entity_id: String },
    /// Clear all manual relationships
    Clear,
}

#[derive(Subcommand, Debug)]
enum RuleCommands {
    /// List all dispatch rules
    List,
    /// Show rule for a conversation
    Show { conversation_id: String },
}

/// Run the CLI
pub async fn run(rms: RelationshipManagerService) -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    execute(cli, rms).await
}

/// Run the CLI with pre-parsed arguments (for binary use)
pub async fn run_with_cli(
    cli: Cli,
    rms: RelationshipManagerService,
) -> Result<(), Box<dyn std::error::Error>> {
    execute(cli, rms).await
}

async fn execute(
    cli: Cli,
    rms: RelationshipManagerService,
) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Status => {
            let status = rms.get_status()?;
            print_status(&status);
        }

        Commands::Clients { cmd } => match cmd {
            ClientCommands::List => {
                let clients = rms.get_clients()?;
                print_clients(&clients);
            }
            ClientCommands::Show { id } => match rms.get_client(&id)? {
                Some(client) => print_client_detail(&client),
                None => println!("Client not found: {id}"),
            },
        },

        Commands::Entities { cmd } => match cmd {
            EntityCommands::List {
                r#type,
                assigned,
                unassigned,
                search,
            } => {
                let has_relationship = if assigned {
                    Some(true)
                } else if unassigned {
                    Some(false)
                } else {
                    None
                };

                let filter = EntityFilter {
                    entity_type: r#type
                        .as_ref()
                        .and_then(|s| LineEntityType::parse_entity_type(s)),
                    has_relationship,
                    search,
                    limit: Some(100),
                    offset: None,
                };

                let entities = rms.get_entities(&filter)?;
                let relationships = rms.get_relationships()?;
                let rel_map: std::collections::HashMap<_, _> = relationships
                    .iter()
                    .map(|r| (r.line_entity_id.as_str(), r))
                    .collect();

                print_entities(&entities, &rel_map);
            }
            EntityCommands::Show { id } => match rms.get_entity(&id).await? {
                Some(entity) => print_entity_detail(&entity),
                None => println!("Entity not found: {id}"),
            },
            EntityCommands::Refresh { id } => match rms.refresh_entity(&id).await? {
                Some(entity) => {
                    println!("Entity refreshed:");
                    print_entity_detail(&entity);
                }
                None => println!("Failed to refresh entity: {id}"),
            },
        },

        Commands::Relationships { cmd } => match cmd {
            RelationshipCommands::List => {
                let relationships = rms.get_relationships()?;
                print_relationships(&relationships);
            }
            RelationshipCommands::Show { entity_id } => {
                match rms.get_relationship(&entity_id)? {
                    Some(rel) => print_relationship_detail(&rel),
                    None => println!("Relationship not found for entity: {entity_id}"),
                }
            }
            RelationshipCommands::Set {
                entity_id,
                client_id,
                notes,
            } => {
                let rel = rms
                    .set_relationship(&entity_id, &client_id, notes.as_deref())
                    .await?;
                println!("✓ Relationship created/updated");
                print_relationship_detail(&rel);
            }
            RelationshipCommands::Remove { entity_id } => {
                if rms.remove_relationship(&entity_id)? {
                    println!("✓ Relationship removed for entity: {entity_id}");
                } else {
                    println!("Relationship not found for entity: {entity_id}");
                }
            }
            RelationshipCommands::Clear => {
                let count = rms.clear_manual_relationships()?;
                println!("✓ Cleared {count} manual relationships");
            }
        },

        Commands::Rules { cmd } => match cmd {
            RuleCommands::List => {
                let rules = rms.get_dispatch_rules()?;
                print_dispatch_rules(&rules);
            }
            RuleCommands::Show { conversation_id } => {
                match rms.get_dispatch_rule(&conversation_id)? {
                    Some(rule) => print_dispatch_rule_detail(&rule),
                    None => println!("Dispatch rule not found for: {conversation_id}"),
                }
            }
        },

        Commands::Import => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;

            let imports: Vec<RelationshipImport> = serde_json::from_str(&input)?;
            let result = rms.import_relationships(&imports).await?;

            println!("Import complete:");
            println!("  Imported: {}", result.imported);
            println!("  Updated: {}", result.updated);
            if !result.errors.is_empty() {
                println!("  Errors:");
                for err in result.errors {
                    println!("    - {err}");
                }
            }
        }

        Commands::Export => {
            let relationships = rms.export_relationships()?;
            let json = serde_json::to_string_pretty(&relationships)?;
            println!("{json}");
        }

        Commands::Sync => {
            let result = rms.sync_ownership()?;
            println!("Sync complete:");
            println!("  Added: {}", result.added);
            println!("  Updated: {}", result.updated);
            println!("  Removed: {}", result.removed);
        }
    }

    Ok(())
}

// ========== Output Formatting ==========

fn print_status(status: &SystemStatus) {
    println!();
    println!("┌─────────────────────────────────────────────────────┐");
    println!("│ UGENT-LINE-PROXY Status                             │");
    println!("├─────────────────────────────────────────────────────┤");
    println!("│ Connected Clients:     {:<27}│", status.connected_clients);
    println!("│ Total Entities:        {:<27}│", status.total_entities);
    println!(
        "│ Total Relationships:   {:<27}│",
        status.total_relationships
    );
    println!(
        "│   ├─ Manual:           {:<27}│",
        status.manual_relationships
    );
    println!(
        "│   └─ Auto-detected:    {:<27}│",
        status.auto_relationships
    );
    println!("│ Pending Messages:      {:<27}│", status.pending_messages);

    let uptime = format_uptime(status.uptime_secs);
    println!("│ Uptime:                {uptime:<27}│");
    println!("└─────────────────────────────────────────────────────┘");
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;

    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

fn print_clients(clients: &[ClientInfo]) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Client ID", "Connected", "Last Activity", "Owned"]);

    for client in clients {
        let connected = if client.connected {
            Cell::new("✓ Yes").fg(Color::Green)
        } else {
            Cell::new("✗ No").fg(Color::Red)
        };

        let last_activity = format_timestamp(client.last_activity);

        table.add_row(vec![
            Cell::new(&client.client_id),
            connected,
            Cell::new(&last_activity),
            Cell::new(client.owned_conversations.to_string()),
        ]);
    }

    println!("{table}");
}

fn print_client_detail(client: &ClientInfo) {
    println!();
    println!("Client: {}", client.client_id);
    println!(
        "  Connected: {}",
        if client.connected { "Yes" } else { "No" }
    );
    println!(
        "  Connected At: {}",
        client
            .connected_at
            .map_or_else(|| "N/A".to_string(), format_timestamp)
    );
    println!(
        "  Last Activity: {}",
        format_timestamp(client.last_activity)
    );
    println!("  Owned Conversations: {}", client.owned_conversations);
    if let Some(metadata) = &client.metadata {
        println!(
            "  Metadata: {}",
            serde_json::to_string(metadata).unwrap_or_default()
        );
    }
}

fn print_entities(
    entities: &[LineEntity],
    rel_map: &std::collections::HashMap<&str, &Relationship>,
) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        "Entity ID",
        "Type",
        "Display Name",
        "Assigned Client",
        "Manual?",
    ]);

    for entity in entities {
        let (assigned_client, is_manual) = match rel_map.get(entity.id.as_str()) {
            Some(rel) => (rel.client_id.as_str().to_string(), rel.is_manual),
            None => ("-".to_string(), false),
        };

        table.add_row(vec![
            Cell::new(&entity.id),
            Cell::new(entity.entity_type.as_str()),
            Cell::new(entity.display_name.as_deref().unwrap_or("-")),
            Cell::new(&assigned_client),
            Cell::new(if is_manual { "✓" } else { "" }),
        ]);
    }

    println!("{table}");
}

fn print_entity_detail(entity: &LineEntity) {
    println!();
    println!("Entity: {}", entity.id);
    println!("  Type: {}", entity.entity_type);
    println!(
        "  Display Name: {}",
        entity.display_name.as_deref().unwrap_or("-")
    );
    println!(
        "  Picture URL: {}",
        entity.picture_url.as_deref().unwrap_or("-")
    );
    println!(
        "  Last Message: {}",
        entity
            .last_message_at
            .map_or_else(|| "N/A".to_string(), format_timestamp)
    );
    println!("  Created: {}", format_timestamp(entity.created_at));
    println!("  Updated: {}", format_timestamp(entity.updated_at));
}

fn print_relationships(relationships: &[Relationship]) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Entity ID", "Type", "Client ID", "Manual", "Notes"]);

    for rel in relationships {
        table.add_row(vec![
            Cell::new(&rel.line_entity_id),
            Cell::new(rel.entity_type.as_str()),
            Cell::new(&rel.client_id),
            Cell::new(if rel.is_manual { "✓" } else { "" }),
            Cell::new(rel.notes.as_deref().unwrap_or("-")),
        ]);
    }

    println!("{table}");
}

fn print_relationship_detail(rel: &Relationship) {
    println!();
    println!("Relationship #{}", rel.id);
    println!("  Entity:     {} ({})", rel.line_entity_id, rel.entity_type);
    println!("  Client:     {}", rel.client_id);
    println!(
        "  Type:       {}",
        if rel.is_manual { "Manual" } else { "Auto" }
    );
    println!("  Created:    {}", format_timestamp(rel.created_at));
    println!("  Updated:    {}", format_timestamp(rel.updated_at));
    if let Some(notes) = &rel.notes {
        println!("  Notes:      {notes}");
    }
}

fn print_dispatch_rules(rules: &[DispatchRule]) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        "Conversation",
        "Assigned Client",
        "Connected",
        "Type",
        "Messages",
    ]);

    for rule in rules {
        let (client, connected) = match &rule.assigned_client {
            Some(c) => (
                c.as_str().to_string(),
                if rule.assigned_client_connected {
                    Cell::new("✓").fg(Color::Green)
                } else {
                    Cell::new("✗").fg(Color::Red)
                },
            ),
            None => ("(broadcast)".to_string(), Cell::new("-")),
        };

        let route_type = if rule.is_manual { "Manual" } else { "Auto" };

        table.add_row(vec![
            Cell::new(&rule.conversation_id),
            Cell::new(&client),
            connected,
            Cell::new(route_type),
            Cell::new(rule.message_count.to_string()),
        ]);
    }

    println!("{table}");
}

fn print_dispatch_rule_detail(rule: &DispatchRule) {
    println!();
    println!("Dispatch Rule: {}", rule.conversation_id);
    println!("  Type: {}", rule.entity_type);
    println!(
        "  Assigned Client: {}",
        rule.assigned_client.as_deref().unwrap_or("(broadcast)")
    );
    println!(
        "  Client Connected: {}",
        if rule.assigned_client_connected {
            "Yes"
        } else {
            "No"
        }
    );
    println!(
        "  Route Type: {}",
        if rule.is_manual { "Manual" } else { "Auto" }
    );
    println!(
        "  Last Routed: {}",
        rule.last_routed_at
            .map_or_else(|| "N/A".to_string(), format_timestamp)
    );
    println!("  Message Count: {}", rule.message_count);
}

fn format_timestamp(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map_or_else(|| ts.to_string(), |dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
}
