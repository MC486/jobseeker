//! Process entrypoint: `jobseeker serve`, `add`, `list`, `show`, `migrate`.

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
    /// Rebuild files from the database (M1). Currently a status check.
    Reconcile,
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
                println!(
                    "{}  {} — {}  [{}]  {}",
                    row.id, row.title, row.company_name, row.work_mode, row.status
                );
            }
            if let Some(c) = page.next_cursor {
                println!("next_cursor={c}");
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
                println!(
                    "match {:.0}% ({})",
                    score.overall * 100.0,
                    if score.is_stale { "stale" } else { "fresh" }
                );
            }
        }
        Command::Reconcile => {
            anyhow::bail!("reconcile is scheduled for M1; files are written on extract today");
        }
    }
    Ok(())
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
