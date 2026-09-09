//! Process entrypoint: `jobseeker serve`, `add`, `list`, `show`, `triage`, `track`, `merge`, `split`, `duplicates`, `conflicts`, `migrate`, `openapi`, `reconcile`, `profile`.

use std::io::{self, Read};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use jobseeker_core::config::Config;
use jobseeker_db::repo::job::{self, JobFilter};
use jobseeker_pipeline::Pipeline;

#[derive(Parser, Debug)]
#[command(
    name = "jobseeker",
    version,
    about = "Self-hosted job-search workbench"
)]
struct Cli {
    /// Path to a TOML config file. Defaults + `JOBSEEKER__*` env still apply.
    #[arg(long, global = true, env = "JOBSEEKER_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the HTTP server and in-process workers.
    Serve {
        /// Override `server.bind`.
        #[arg(long)]
        bind: Option<String>,
    },
    /// Apply pending SQLite migrations and exit.
    Migrate,
    /// Ingest a URL or pasted HTML, then drain the queue so the job exists before return.
    Add {
        /// Public job URL. LinkedIn/Indeed are refused — use the extension.
        url: Option<String>,
        /// Read HTML/text from a file, or `-` for stdin.
        #[arg(long)]
        paste: Option<PathBuf>,
        /// Do not wait for extraction to finish.
        #[arg(long)]
        async_queue: bool,
    },
    /// List saved jobs.
    List {
        #[arg(long)]
        query: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Show one job by id.
    Show { id: String },
    /// Set posting status, star rating, or notes. This is not an application pipeline.
    Triage {
        id: String,
        /// Posting lifecycle: open, closed, filled, expired, removed.
        #[arg(long)]
        status: Option<String>,
        /// 0–5. Your judgment, not the match score.
        #[arg(long)]
        rating: Option<i32>,
        /// Replace `user_notes_md`.
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        archive: bool,
        #[arg(long)]
        unarchive: bool,
    },
    /// Application pipeline status and next action.
    Track {
        id: String,
        /// interested|preparing|applied|screening|interviewing|offer|accepted|rejected|withdrawn|ghosted
        #[arg(long)]
        status: Option<String>,
        /// What you will do next (empty string clears).
        #[arg(long = "next")]
        next_action: Option<String>,
        /// Calendar date YYYY-MM-DD (empty string clears).
        #[arg(long)]
        due: Option<String>,
    },
    /// Merge a cross-post into another job of the same company.
    Merge {
        /// Job to absorb (soft-deleted).
        from: String,
        /// Job that keeps its title and description.
        into: String,
    },
    /// Peel one listing off a merged job into a new job.
    Split {
        /// Job that currently holds the listing.
        from: String,
        /// Listing to peel off.
        listing: String,
    },
    /// Flag same-company title matches. Never merges.
    Duplicates { id: String },
    /// List extraction conflicts for a job (salary disagreements after merge).
    Conflicts { id: String },
    /// Resolve one conflict by picking A, B, or the current value.
    ResolveConflict {
        /// Job that owns the conflict.
        job: String,
        /// Conflict id from `conflicts`.
        conflict: String,
        /// `a`, `b`, or `keep`.
        #[arg(long)]
        pick: String,
    },
    /// Pair a browser extension: print a one-time code, or emit a device token.
    Pair {
        /// Label stored with the token, e.g. "Firefox on laptop".
        #[arg(long)]
        name: String,
        /// Print a `jst_…` token immediately instead of a pairing code.
        #[arg(long)]
        emit_token: bool,
    },
    /// List device tokens (never the secret).
    Tokens,
    /// Revoke a device token by id.
    Revoke { id: String },
    /// Keep SQLite and `jobs/**` in sync (ADR-0003).
    Reconcile {
        /// Rewrite `job.md` / `job.json` / `requirements.json` from the database.
        #[arg(long)]
        to_files: bool,
        /// Rebuild job rows from `jobs/**/job.json` (skips tombstones).
        #[arg(long)]
        from_files: bool,
        /// Report db_only / file_only / divergent and change nothing.
        #[arg(long)]
        check: bool,
    },
    /// Dump the OpenAPI document (no database, no server).
    Openapi {
        /// Write JSON to this path instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Create or rotate the owner password used when `auth.mode = password`.
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Default profile and experience bank.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
}

#[derive(Subcommand, Debug)]
enum UserCommand {
    /// Hash a password with Argon2id and store it on the owner account.
    SetPassword {
        #[arg(long, default_value = "owner")]
        username: String,
        /// If omitted, read a single line from stdin.
        #[arg(long)]
        password: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum ProfileCommand {
    /// Parse a Markdown evidence bank or resume into the default profile.
    Import {
        /// Markdown file, or `-` for stdin.
        #[arg(long)]
        file: PathBuf,
        /// Do not wait for queued rescores to finish.
        #[arg(long)]
        async_queue: bool,
    },
    /// Print the default profile as JSON.
    Show,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing();
    let mut config = Config::load(cli.config.as_deref()).unwrap_or_else(|e| {
        if cli.config.is_some() {
            eprintln!("config error: {e}");
            std::process::exit(2);
        }
        Config::default()
    });

    match cli.command {
        Command::Serve { bind } => {
            if let Some(bind) = bind {
                config.server.bind = bind;
            }
            let pipeline = Pipeline::open(config).await?;
            let workers = pipeline.config.worker_concurrency();
            let (tx, rx) = tokio::sync::watch::channel(false);
            for i in 0..workers {
                let p = pipeline.clone();
                let rx = rx.clone();
                tokio::spawn(async move {
                    tracing::info!(worker = i, "worker started");
                    p.run_worker(rx).await;
                });
            }
            let result = jobseeker_api::serve(pipeline).await;
            let _ = tx.send(true);
            result?;
        }
        Command::Migrate => {
            let pipeline = Pipeline::open(config).await?;
            pipeline.db.migrate().await?;
            println!(
                "migrations applied; schema version {:?}",
                pipeline.db.applied_version().await?
            );
        }
        Command::Add {
            url,
            paste,
            async_queue,
        } => {
            let pipeline = Pipeline::open(config).await?;
            let accepted = if let Some(path) = paste {
                let text = read_paste(&path)?;
                pipeline.ingest_paste(&text, url.as_deref()).await?
            } else {
                let url = url.context("pass a URL or --paste <file>")?;
                pipeline.ingest_url(&url).await?
            };
            println!("task {}", accepted.task_id);
            if let Some(id) = &accepted.listing_id {
                println!("listing {id}");
            }
            if let Some(id) = &accepted.capture_id {
                println!("capture {id}");
            }
            if !async_queue {
                let n = pipeline.drain().await?;
                println!("processed {n} task(s)");
                let page = job::list(
                    &pipeline.db,
                    &JobFilter {
                        limit: Some(1),
                        ..Default::default()
                    },
                )
                .await?;
                if let Some(row) = page.items.first() {
                    println!("{} — {} ({})", row.title, row.company_name, row.id);
                }
            }
        }
        Command::List { query, limit } => {
            let pipeline = Pipeline::open(config).await?;
            let page = job::list(
                &pipeline.db,
                &JobFilter {
                    query,
                    limit: Some(limit),
                    ..Default::default()
                },
            )
            .await?;
            for row in page.items {
                print!(
                    "{}  {} — {}  [{}]  {}",
                    row.id, row.title, row.company_name, row.work_mode, row.status
                );
                if let Some(overall) = row.match_overall {
                    print!("  {:.0}%", overall * 100.0);
                    if let Some(skills) = row.skills_coverage {
                        print!("  skills {:.0}%", skills * 100.0);
                    }
                    if let Some(years) = row.years_fit {
                        print!("  years {:.0}%", years * 100.0);
                    }
                }
                if let Some(app) = &row.application_status {
                    print!("  pipeline {app}");
                }
                if let Some(due) = &row.next_action_due {
                    print!("  due {due}");
                }
                if let Some(act) = &row.next_action {
                    print!("  next {act}");
                }
                println!();
            }
            if let Some(c) = page.next_cursor {
                println!("next_cursor={c}");
            }
        }
        Command::Duplicates { id } => {
            let pipeline = Pipeline::open(config).await?;
            let id: jobseeker_core::ids::JobId = id.parse()?;
            let found = job::duplicates(&pipeline.db, &id).await?;
            if found.is_empty() {
                println!("no same-company title matches");
            }
            for c in found {
                println!(
                    "{}  {} — {}  title {:.0}%  desc {:.0}%  {}  {} reqs{}",
                    c.job_id,
                    c.title,
                    c.company_name,
                    c.title_jaccard * 100.0,
                    c.description_cosine * 100.0,
                    c.strength,
                    c.requirement_count,
                    if c.keep_this { "  keep this" } else { "" }
                );
            }
        }
        Command::Conflicts { id } => {
            let pipeline = Pipeline::open(config).await?;
            let id: jobseeker_core::ids::JobId = id.parse()?;
            let found = jobseeker_db::repo::conflict::list_for_job(&pipeline.db, &id).await?;
            if found.is_empty() {
                println!("no extraction conflicts");
            }
            for c in found {
                let sides = format!(
                    "{}/{}",
                    c.provenance_a.as_deref().unwrap_or("a"),
                    c.provenance_b.as_deref().unwrap_or("b")
                );
                println!(
                    "{}  {}  {}  a={}  b={}  resolved={}  {sides}",
                    c.id,
                    c.field,
                    c.resolution,
                    c.value_a.as_deref().unwrap_or("-"),
                    c.value_b.as_deref().unwrap_or("-"),
                    c.resolved_value.as_deref().unwrap_or("-")
                );
            }
        }
        Command::ResolveConflict {
            job,
            conflict,
            pick,
        } => {
            let pipeline = Pipeline::open(config).await?;
            let job: jobseeker_core::ids::JobId = job.parse()?;
            let choice: jobseeker_db::repo::conflict::ResolveChoice = pick.parse()?;
            let row = pipeline.resolve_conflict(&job, &conflict, choice).await?;
            println!(
                "resolved {}  {}  {}  value={}",
                row.id,
                row.field,
                row.resolution,
                row.resolved_value.as_deref().unwrap_or("-")
            );
        }
        Command::Split { from, listing } => {
            let pipeline = Pipeline::open(config).await?;
            let from: jobseeker_core::ids::JobId = from.parse()?;
            let listing: jobseeker_core::ids::ListingId = listing.parse()?;
            let report = pipeline.split_job(&from, &listing).await?;
            println!(
                "split {} off {}  new_job={}",
                report.listing_id, report.from_id, report.new_id
            );
        }
        Command::Merge { from, into } => {
            let pipeline = Pipeline::open(config).await?;
            let from: jobseeker_core::ids::JobId = from.parse()?;
            let into: jobseeker_core::ids::JobId = into.parse()?;
            let report = pipeline.merge_jobs(&from, &into).await?;
            println!(
                "merged {} into {}  listings_moved={}  requirements_added={}",
                report.from_id, report.into_id, report.listings_moved, report.requirements_added
            );
        }
        Command::Triage {
            id,
            status,
            rating,
            note,
            archive,
            unarchive,
        } => {
            if archive && unarchive {
                anyhow::bail!("pass --archive or --unarchive, not both");
            }
            let pipeline = Pipeline::open(config).await?;
            let id: jobseeker_core::ids::JobId = id.parse()?;
            let changing =
                status.is_some() || rating.is_some() || note.is_some() || archive || unarchive;
            if changing {
                job::patch(
                    &pipeline.db,
                    &id,
                    &job::JobPatch {
                        status,
                        user_rating: rating,
                        user_notes_md: note,
                        is_archived: if archive {
                            Some(true)
                        } else if unarchive {
                            Some(false)
                        } else {
                            None
                        },
                        ..Default::default()
                    },
                )
                .await?;
            }
            let job = job::get(&pipeline.db, &id)
                .await?
                .with_context(|| format!("no job {id}"))?;
            println!(
                "{}  {}  rating={}  archived={}  notes={}",
                job.status,
                job.title,
                job.user_rating
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".into()),
                job.is_archived,
                job.user_notes_md.as_deref().unwrap_or("-")
            );
        }
        Command::Track {
            id,
            status,
            next_action,
            due,
        } => {
            let pipeline = Pipeline::open(config).await?;
            let id: jobseeker_core::ids::JobId = id.parse()?;
            if status.is_some() || next_action.is_some() || due.is_some() {
                let status = match status {
                    Some(s) => Some(s.parse()?),
                    None => None,
                };
                let row = pipeline
                    .patch_application(
                        &id,
                        jobseeker_db::repo::application::ApplicationPatch {
                            status,
                            next_action,
                            next_action_due: due,
                        },
                    )
                    .await?;
                print_application(&row);
            } else {
                match jobseeker_db::repo::application::get_for_job(&pipeline.db, &id).await? {
                    Some(row) => print_application(&row),
                    None => println!("no application yet"),
                }
            }
        }
        Command::Show { id } => {
            let pipeline = Pipeline::open(config).await?;
            let id: jobseeker_core::ids::JobId = id.parse()?;
            let job = job::get(&pipeline.db, &id)
                .await?
                .with_context(|| format!("no job {id}"))?;
            println!("{}", serde_json::to_string_pretty(&job)?);
            if let Some(score) =
                jobseeker_db::repo::score::latest_for_job(&pipeline.db, &id, None).await?
            {
                print!(
                    "match {:.0}% ({})",
                    score.overall * 100.0,
                    if score.is_stale { "stale" } else { "fresh" }
                );
                if let Some(skills) = score.skills_coverage {
                    print!("  skills {:.0}%", skills * 100.0);
                }
                if let Some(years) = score.years_fit {
                    print!("  years {:.0}%", years * 100.0);
                }
                println!();
            }
        }
        Command::Pair { name, emit_token } => {
            let pipeline = Pipeline::open(config).await?;
            let server = pipeline
                .config
                .server
                .public_url
                .clone()
                .unwrap_or_else(|| format!("http://{}", pipeline.config.server.bind));
            if emit_token {
                let issued =
                    jobseeker_db::repo::token::issue_token(&pipeline.db, &name, &["ingest".into()])
                        .await?;
                println!("Paste this into the extension (shown once; stored hashed):");
                println!("  server: {server}");
                println!("  token:  {}", issued.token);
            } else {
                let code =
                    jobseeker_db::repo::token::create_pairing_code(&pipeline.db, &name).await?;
                println!("Pairing code (expires in 15 minutes):");
                println!("  {}", code.code);
                println!();
                println!("Paste the code into the extension, or re-run with --emit-token.");
                println!("  server: {server}");
            }
        }
        Command::Tokens => {
            let pipeline = Pipeline::open(config).await?;
            for row in jobseeker_db::repo::token::list(&pipeline.db).await? {
                let state = if row.revoked_at.is_some() {
                    "revoked"
                } else {
                    "active"
                };
                println!(
                    "{}  {}  [{}]  {state}",
                    row.id,
                    row.name,
                    row.scopes.join(",")
                );
            }
        }
        Command::Revoke { id } => {
            let pipeline = Pipeline::open(config).await?;
            let n = jobseeker_db::repo::token::revoke(&pipeline.db, &id).await?;
            if n == 0 {
                anyhow::bail!("no token {id}");
            }
            println!("revoked {id}");
        }
        Command::Reconcile {
            to_files,
            from_files,
            check,
        } => {
            if to_files && from_files {
                anyhow::bail!("choose one of --to-files or --from-files");
            }
            let pipeline = Pipeline::open(config).await?;
            let report = if to_files {
                pipeline.reconcile_to_files().await?
            } else if from_files {
                pipeline.reconcile_from_files().await?
            } else {
                pipeline.reconcile_check().await?
            };
            println!(
                "jobs db={} files={} written={} restored={} skipped_tombstone={}",
                report.db_jobs,
                report.file_jobs,
                report.written,
                report.restored,
                report.skipped_tombstone
            );
            if !report.db_only.is_empty() {
                println!("db_only: {}", report.db_only.join(" "));
            }
            if !report.file_only.is_empty() {
                println!("file_only: {}", report.file_only.join(" "));
            }
            if !report.divergent.is_empty() {
                println!("divergent: {}", report.divergent.join(" "));
            }
            if report.is_clean() {
                println!("drift: none");
            } else if check || (!to_files && !from_files) {
                std::process::exit(1);
            }
        }
        Command::User { command } => match command {
            UserCommand::SetPassword { username, password } => {
                let pipeline = Pipeline::open(config).await?;
                let password = match password {
                    Some(p) => p,
                    None => {
                        eprint!("password: ");
                        let mut buf = String::new();
                        io::stdin().read_line(&mut buf)?;
                        buf.trim_end_matches(['\n', '\r']).to_string()
                    }
                };
                let user =
                    jobseeker_db::repo::user::upsert_owner(&pipeline.db, &username, &password)
                        .await?;
                println!("owner {} ready ({})", user.username, user.id);
            }
        },
        Command::Profile { command } => match command {
            ProfileCommand::Import { file, async_queue } => {
                let pipeline = Pipeline::open(config).await?;
                let text = read_paste(&file)?;
                let report = pipeline.import_resume(&text).await?;
                println!(
                    "profile {}  items={} accomplishments={} skills={}",
                    report.profile_id, report.items, report.accomplishments, report.skills
                );
                if !async_queue {
                    let n = pipeline.drain().await?;
                    println!("processed {n} task(s)");
                }
            }
            ProfileCommand::Show => {
                let pipeline = Pipeline::open(config).await?;
                let id = jobseeker_db::repo::profile::ensure_default(&pipeline.db).await?;
                let view = jobseeker_db::repo::experience::get_view(&pipeline.db, &id)
                    .await?
                    .context("no default profile")?;
                println!("{}", serde_json::to_string_pretty(&view)?);
            }
        },
        Command::Openapi { out } => {
            let spec = jobseeker_api::openapi_spec();
            let pretty = serde_json::to_string_pretty(&spec)?;
            if let Some(path) = out {
                std::fs::write(&path, pretty)
                    .with_context(|| format!("write {}", path.display()))?;
            } else {
                println!("{pretty}");
            }
        }
    }
    Ok(())
}

fn print_application(row: &jobseeker_db::repo::application::ApplicationView) {
    print!(
        "{}  {}  applied_at={}",
        row.status,
        row.job_id,
        row.applied_at.as_deref().unwrap_or("-")
    );
    if let Some(due) = &row.next_action_due {
        print!("  due={due}");
    } else {
        print!("  due=-");
    }
    if let Some(act) = &row.next_action {
        print!("  next={act}");
    } else {
        print!("  next=-");
    }
    println!();
}

fn read_paste(path: &PathBuf) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        return Ok(buf);
    }
    Ok(std::fs::read_to_string(path)?)
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
}
