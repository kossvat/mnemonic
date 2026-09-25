//! Explicitly published project knowledge and an untrusted observations inbox.
//!
//! This subsystem never opens private memory storage or fans out to its sinks.
//! Authorization for stdio comes from a trusted launcher and a fixed policy;
//! it is not a sandbox against a process that can read the hub's filesystem.

pub mod access;
pub mod curation;
mod filesystem;
mod mcp;
mod schema;
pub mod store;
pub mod types;

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use std::io::Read;
use std::path::{Path, PathBuf};
use store::SharedStore;
use types::Expected;

/// What the curator believes the key holds right now.
#[derive(Debug, Args)]
pub struct ExpectArgs {
    /// Fail unless the key does not exist yet
    #[arg(long, conflicts_with = "expect_revision")]
    expect_absent: bool,
    /// Fail unless the key is still at this revision
    #[arg(long, value_name = "N")]
    expect_revision: Option<i64>,
}

impl ExpectArgs {
    fn expected(&self) -> Expected {
        match (self.expect_absent, self.expect_revision) {
            (true, _) => Expected::Absent,
            (false, Some(revision)) => Expected::Revision(revision),
            (false, None) => Expected::Any,
        }
    }
}

#[derive(Debug, Args)]
pub struct SharedArgs {
    /// Explicit path to a dedicated shared SQLite DB; never private memory.db
    #[arg(long)]
    pub db: PathBuf,
    #[command(subcommand)]
    pub command: SharedCommand,
}

#[derive(Debug, Subcommand)]
pub enum SharedCommand {
    /// Owner operation: publish reviewed text from a file (or - for stdin)
    Publish {
        #[arg(long)]
        project: String,
        #[arg(long)]
        key: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        file: PathBuf,
        /// Source reference safe to disclose to this project's agents
        #[arg(long)]
        source: String,
        #[command(flatten)]
        expect: ExpectArgs,
        /// Who approved this text (attribution, not authentication)
        #[arg(long)]
        actor: Option<String>,
    },
    /// Owner operation: withdraw a published key and advance project revision
    Revoke {
        #[arg(long)]
        project: String,
        #[arg(long)]
        key: String,
        #[command(flatten)]
        expect: ExpectArgs,
    },
    /// Owner operation: publish the exact text of ONE observation you have read
    Promote {
        #[arg(long)]
        project: String,
        /// Observation id from `inbox`
        #[arg(long)]
        id: String,
        #[arg(long)]
        key: String,
        #[command(flatten)]
        expect: ExpectArgs,
        /// Who approved this text (attribution, not authentication)
        #[arg(long)]
        actor: String,
    },
    /// Owner operation: bind this database to a single project for good
    Pin {
        #[arg(long)]
        project: String,
    },
    /// Owner operation: cut off, restore or list people with revoked access
    Access {
        #[command(subcommand)]
        command: AccessCommand,
    },
    /// Owner operation: render or check the SSH side of hub access
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// Owner operation: read the project's published context as JSON
    Context {
        #[arg(long)]
        project: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Owner operation: list untrusted pending observations as JSON
    Inbox {
        #[arg(long)]
        project: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Owner operation: mark an observation reviewed or rejected; does not publish
    Review {
        #[arg(long)]
        project: String,
        #[arg(long)]
        id: String,
        #[arg(long, value_parser = ["reviewed", "rejected"])]
        status: String,
    },
    /// Restricted stdio MCP: fixed project/agent policy, no owner tools
    Serve {
        /// Trusted launcher-owned TOML policy; not writable by the agent
        #[arg(long)]
        policy: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// Print the policy file and the one authorized_keys line for an agent.
    /// Nothing is installed: an administrator writes both where they belong.
    Grant {
        #[arg(long)]
        project: String,
        /// The person this agent acts for; revoking them cuts off all of theirs
        #[arg(long)]
        principal: String,
        /// Short agent name, for example claude, codex or contrib
        #[arg(long)]
        agent: String,
        /// Absolute path of the mnemonic binary the forced command runs
        #[arg(long)]
        bin: PathBuf,
        /// Directory holding this hub's policy files
        #[arg(long)]
        policy_dir: PathBuf,
        /// File with exactly one public key
        #[arg(long)]
        public_key: PathBuf,
        /// Let this agent file observations for review (default: read only)
        #[arg(long)]
        write: bool,
        /// RFC 3339 instant after which the policy stops working
        #[arg(long)]
        expires_at: Option<String>,
        /// End a session that has been open this many seconds
        #[arg(long)]
        max_session_secs: Option<u64>,
    },
    /// Check an authorized_keys file; exits non-zero when anything is unsafe
    Lint {
        #[arg(long)]
        authorized_keys: PathBuf,
        #[arg(long)]
        bin: PathBuf,
        #[arg(long)]
        policy_dir: PathBuf,
        /// File with the curator's own login keys, which may be unrestricted
        #[arg(long)]
        owner_keys: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AccessCommand {
    /// Every session of this person's agents fails on its next request
    Revoke {
        #[arg(long)]
        project: String,
        #[arg(long)]
        principal: String,
        #[arg(long)]
        actor: Option<String>,
    },
    Restore {
        #[arg(long)]
        project: String,
        #[arg(long)]
        principal: String,
    },
    List {
        #[arg(long)]
        project: String,
    },
}

/// Bounded UTF-8 input for policy and published text; no unbounded read_to_string.
fn read_text(path: &Path, limit: usize) -> Result<String> {
    let reader: Box<dyn Read> = if path == Path::new("-") {
        Box::new(std::io::stdin())
    } else {
        let file = std::fs::File::open(path).context("opening shared input file")?;
        ensure!(
            file.metadata()?.is_file(),
            "shared input must be a regular file"
        );
        Box::new(file)
    };
    read_bounded_text(reader, limit)
}

fn read_bounded_text(reader: impl Read, limit: usize) -> Result<String> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "shared input exceeds {limit} bytes");
    String::from_utf8(bytes).context("shared input must be UTF-8")
}

pub fn run(args: &SharedArgs) -> Result<()> {
    ensure!(
        args.db.is_absolute(),
        "--db must be an explicit absolute path"
    );
    // Validate the entire fixed policy before opening or creating any database.
    let policy = if let SharedCommand::Serve { policy } = &args.command {
        ensure!(policy.is_absolute(), "--policy must be an absolute path");
        Some(mcp::Policy::load(policy)?)
    } else {
        None
    };
    let store = SharedStore::open(&args.db)?;
    let result = match &args.command {
        SharedCommand::Publish {
            project,
            key,
            title,
            file,
            source,
            expect,
            actor,
        } => {
            let content = read_text(file, 32768)?;
            serde_json::to_value(store.publish_checked(
                project,
                key,
                title,
                &content,
                source,
                expect.expected(),
                actor.as_deref(),
            )?)?
        }
        SharedCommand::Revoke {
            project,
            key,
            expect,
        } => {
            serde_json::json!({"revoked": store.revoke_checked(project, key, expect.expected())?})
        }
        SharedCommand::Promote {
            project,
            id,
            key,
            expect,
            actor,
        } => serde_json::to_value(store.promote(project, id, key, expect.expected(), actor)?)?,
        SharedCommand::Pin { project } => {
            store.pin_project(project)?;
            serde_json::json!({"pinned_project": store.pinned_project()?})
        }
        SharedCommand::Keys { command } => match command {
            KeysCommand::Grant {
                project,
                principal,
                agent,
                bin,
                policy_dir,
                public_key,
                write,
                expires_at,
                max_session_secs,
            } => {
                let key = read_text(public_key, 16384)?;
                let grant = access::grant(&access::GrantRequest {
                    db: &args.db,
                    bin,
                    policy_dir,
                    project,
                    principal,
                    agent,
                    write: *write,
                    expires_at: expires_at.as_deref(),
                    max_session_secs: *max_session_secs,
                    public_key: &key,
                })?;
                serde_json::to_value(&grant)?
            }
            KeysCommand::Lint {
                authorized_keys,
                bin,
                policy_dir,
                owner_keys,
            } => {
                let keys = read_text(authorized_keys, 262_144)?;
                let owner = match owner_keys {
                    Some(path) => read_text(path, 65_536)?
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty() && !line.starts_with('#'))
                        .map(str::to_owned)
                        .collect(),
                    None => Vec::new(),
                };
                let problems = access::lint(&access::LintRequest {
                    db: &args.db,
                    bin,
                    policy_dir,
                    authorized_keys: &keys,
                    owner_keys: &owner,
                    pinned_project: store.pinned_project()?.as_deref(),
                })?;
                // A lint that only prints is a lint nobody notices.
                ensure!(
                    problems.is_empty(),
                    "unsafe authorized_keys:\n  {}",
                    problems.join("\n  ")
                );
                serde_json::json!({"problems": problems})
            }
        },
        SharedCommand::Access { command } => match command {
            AccessCommand::Revoke {
                project,
                principal,
                actor,
            } => serde_json::json!({
                "revoked": store.revoke_principal(project, principal, actor.as_deref())?
            }),
            AccessCommand::Restore { project, principal } => {
                serde_json::json!({"restored": store.restore_principal(project, principal)?})
            }
            AccessCommand::List { project } => {
                serde_json::json!({"revocations": store.revocations(project)?})
            }
        },
        SharedCommand::Context { project, limit } => {
            serde_json::to_value(store.context(project, *limit)?)?
        }
        SharedCommand::Inbox { project, limit } => {
            serde_json::json!({"trust": "untrusted_observations", "observations": store.inbox(project, *limit)?})
        }
        SharedCommand::Review {
            project,
            id,
            status,
        } => {
            serde_json::json!({"changed": store.review(project, id, status)?})
        }
        SharedCommand::Serve { .. } => {
            let mut server = mcp::SharedMcp::new(&store, policy.expect("validated serve policy"));
            return server.serve(std::io::stdin().lock(), std::io::stdout().lock());
        }
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
