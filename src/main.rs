use clap::Parser;
use codex_mimo_adapter::cli::{AuthCommands, Cli, Commands, RunArgs};
use codex_mimo_adapter::config::{
    Config, ConfigOverrides, DEFAULT_HOST, DEFAULT_MAX_CONCURRENCY, DEFAULT_PORT,
};
use codex_mimo_adapter::init::run_init;
use codex_mimo_adapter::project::{
    current_environment, read_project_env, registry_dir_path, sign_adapter_token, ProjectPaths,
    ProjectRegistry, PROJECT_ENV_FILENAME,
};
use codex_mimo_adapter::server::{router, AppState, ProjectRuntime};
use codex_mimo_adapter::state::StateStore;
use codex_mimo_adapter::upstream::MimoClient;
use std::sync::{Arc, RwLock};
use tokio::sync::Semaphore;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("codex_mimo_adapter=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Init(args) => run_init(args),
        Commands::Run(args) | Commands::Start(args) => run_server(args).await,
        Commands::Check => run_check().await,
        Commands::Auth(args) => match args.command {
            AuthCommands::PrintLocalToken => {
                let token = load_adapter_token_secret()?;
                println!("{}", sign_adapter_token(&token));
                Ok(())
            }
        },
    }
}

async fn run_server(args: RunArgs) -> anyhow::Result<()> {
    let reg_dir = registry_dir_path()?;
    let registry = ProjectRegistry::load(&reg_dir);
    if registry.projects.is_empty() {
        return Err(anyhow::anyhow!(
            "No projects found in registry. Run 'codex-mimo-adapter init' first."
        ));
    }

    // Shared config overrides used during startup and runtime refresh.
    let config_overrides = ConfigOverrides {
        host: args.host.clone(),
        port: args.port,
        upstream_base: args.upstream_base.clone(),
        upstream_key: args.upstream_key.clone(),
        local_token: args.local_token.clone(),
        state_db: args.state_db.clone(),
        state_ttl_seconds: args.state_ttl_seconds,
        timeout_seconds: args.timeout_seconds,
        max_request_bytes: args.max_request_bytes,
        max_concurrency: args.max_concurrency,
    };

    let mut projects = std::collections::HashMap::new();
    for (project_id, entry) in &registry.projects {
        let root = std::path::PathBuf::from(&entry.root);
        let env_path = root.join(PROJECT_ENV_FILENAME);
        if !env_path.exists() {
            tracing::warn!(
                "project {project_id} missing env file at {}, skipping",
                env_path.display()
            );
            continue;
        }
        let project_env = read_project_env(&env_path)?;
        let env = current_environment();
        let config = Config::from_sources(&project_env, &env, config_overrides.clone())?;
        let state_db_path = root.join(&config.state_db);
        let state = StateStore::new(
            state_db_path.display().to_string(),
            config.state_ttl_seconds,
        )?;
        let client = MimoClient::new(
            &config.upstream_base,
            &config.upstream_key,
            config.timeout_seconds,
        )?;
        tracing::info!(
            "loaded project {project_id} with upstream_base={}",
            config.upstream_base
        );
        projects.insert(
            project_id.clone(),
            ProjectRuntime {
                config,
                client,
                state,
            },
        );
    }
    if projects.is_empty() {
        return Err(anyhow::anyhow!(
            "No valid projects could be loaded from the registry."
        ));
    }
    let host = args
        .host
        .clone()
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    let port = args.port.unwrap_or(DEFAULT_PORT);
    let max_concurrency = args.max_concurrency.unwrap_or(DEFAULT_MAX_CONCURRENCY);
    let addr: std::net::SocketAddr = format!("{}:{}", host, port).parse()?;
    tracing::info!(max_concurrency, "adapter concurrency limit configured");
    let app_state = AppState {
        projects: Arc::new(RwLock::new(projects)),
        capacity: Arc::new(Semaphore::new(max_concurrency)),
        config_overrides,
    };
    let app = router(app_state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn run_check() -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    // Phase 1: Resolve project context for host/port/token.
    // If no project context is available, fall back to defaults for a basic health check.
    let (base, config) = match load_project_config(RunArgs::default()) {
        Ok(config) => {
            let base = format!("http://{}:{}", config.host, config.port);
            (base, Some(config))
        }
        Err(error) => {
            eprintln!("Warning: could not load project config: {error}");
            let base = format!("http://{}:{}", DEFAULT_HOST, DEFAULT_PORT);
            (base, None)
        }
    };

    // Phase 2: Health check works without project context.
    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .map_err(|_| {
            if config.is_some() {
                anyhow::anyhow!(
                    "Adapter is not running at {base}. Start it with 'codex-mimo-adapter run' or 'codex-mimo-adapter start'."
                )
            } else {
                anyhow::anyhow!(
                    "Could not reach adapter at {base}.\n\
                     Either start the adapter, or run from a project directory / set CODEX_MIMO_PROJECT_ID\n\
                     to check the correct host/port from your project configuration."
                )
            }
        })?;
    anyhow::ensure!(
        health.status().is_success(),
        "health check failed at {base}"
    );
    println!("\u{2713} Adapter health check passed at {base}");

    // Phase 3: Models check requires project context.
    match config {
        Some(config) => {
            let raw_token = config
                .local_token
                .as_deref()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("CODEX_MIMO_LOCAL_TOKEN is missing in project config")
                })?;
            let signed_token = sign_adapter_token(raw_token);

            let models = client
                .get(format!("{base}/v1/models"))
                .bearer_auth(&signed_token)
                .send()
                .await?;
            anyhow::ensure!(models.status().is_success(), "/v1/models check failed");
            println!("\u{2713} Models endpoint verified");
            println!("Adapter check passed.");
        }
        None => {
            println!("\u{2713} Adapter is reachable.");
            println!("  For full verification including /v1/models, run from a project directory");
            println!("  or set CODEX_MIMO_PROJECT_ID to your project ID.");
        }
    }

    Ok(())
}

fn load_adapter_token_secret() -> anyhow::Result<String> {
    if let Ok(cwd) = std::env::current_dir() {
        let paths = ProjectPaths::discover_from(&cwd);
        if paths.env_file.exists() {
            let project_env = read_project_env(&paths.env_file)?;
            if let Some(token) = project_env
                .get("CODEX_MIMO_LOCAL_TOKEN")
                .filter(|value| !value.is_empty())
            {
                return Ok(token.to_string());
            }
        }
    }

    let registry = ProjectRegistry::load(&registry_dir_path()?);
    let mut project_ids = registry.projects.keys().cloned().collect::<Vec<_>>();
    project_ids.sort();
    for project_id in project_ids {
        let Some(root) = registry.resolve_root(&project_id) else {
            continue;
        };
        let env_path = root.join(PROJECT_ENV_FILENAME);
        if !env_path.exists() {
            continue;
        }
        let project_env = read_project_env(&env_path)?;
        if let Some(token) = project_env
            .get("CODEX_MIMO_LOCAL_TOKEN")
            .filter(|value| !value.is_empty())
        {
            return Ok(token.to_string());
        }
    }

    Err(anyhow::anyhow!(
        "CODEX_MIMO_LOCAL_TOKEN is missing. Run 'codex-mimo-adapter init' from a project root first."
    ))
}

fn load_project_config(args: RunArgs) -> anyhow::Result<Config> {
    let project = ProjectPaths::from_current_dir()?;
    anyhow::ensure!(
        project.env_file.exists(),
        "Project is not initialized. Run 'codex-mimo-adapter init' from the project root first."
    );
    let project_env = read_project_env(&project.env_file)?;
    // local_token must come only from CLI args or project .env file;
    // strip from process env to prevent accidental pollution.
    let mut env = current_environment();
    env.remove("CODEX_MIMO_LOCAL_TOKEN");
    let overrides = ConfigOverrides {
        host: args.host,
        port: args.port,
        upstream_base: args.upstream_base,
        upstream_key: args.upstream_key,
        local_token: args.local_token,
        state_db: args.state_db,
        state_ttl_seconds: args.state_ttl_seconds,
        timeout_seconds: args.timeout_seconds,
        max_request_bytes: args.max_request_bytes,
        max_concurrency: args.max_concurrency,
    };
    Config::from_sources(&project_env, &env, overrides)
}
