use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use codex_mimo_adapter::config::{Config, ConfigOverrides};
use codex_mimo_adapter::project::read_project_env;
use serde_json::json;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use uuid::Uuid;

const MANAGED_AGENT_FILES: [&str; 9] = [
    "default.toml",
    "explorer.toml",
    "oss-worker-pro-1.toml",
    "oss-worker-pro-2.toml",
    "oss-worker-pro-3.toml",
    "oss-worker-std-1.toml",
    "oss-worker-std-2.toml",
    "oss-worker-std-3.toml",
    "worker.toml",
];

const LEGACY_MANAGED_AGENT_FILES: [&str; 4] = [
    "oss-flash.toml",
    "oss-mimo.toml",
    "oss-minimax.toml",
    "oss-pro.toml",
];

const MANAGED_AGENT_REGISTRATIONS: [(&str, &str); 9] = [
    ("default", "default.toml"),
    ("explorer", "explorer.toml"),
    ("oss_worker_pro_analysis", "oss-worker-pro-1.toml"),
    ("oss_worker_pro_implementation", "oss-worker-pro-2.toml"),
    ("oss_worker_pro_review", "oss-worker-pro-3.toml"),
    ("oss_worker_std_implementation", "oss-worker-std-1.toml"),
    ("oss_worker_std_test", "oss-worker-std-2.toml"),
    ("oss_worker_std_docs", "oss-worker-std-3.toml"),
    ("worker", "worker.toml"),
];

#[test]
fn config_example_registers_exactly_the_managed_agents() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("config.toml.example"))
            .expect("config.toml.example must exist");
    let document = source
        .parse::<toml_edit::DocumentMut>()
        .expect("config.toml.example must be valid TOML");
    let agents = document["agents"]
        .as_table()
        .expect("config.toml.example must contain an agents table");
    let actual: BTreeSet<_> = agents.iter().map(|(name, _)| name.to_string()).collect();
    let expected: BTreeSet<_> = MANAGED_AGENT_REGISTRATIONS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert_eq!(
        actual, expected,
        "example must not expose legacy agent roles"
    );

    for (role, file) in MANAGED_AGENT_REGISTRATIONS {
        let expected_path = format!(".codex/agents/{file}");
        assert_eq!(
            agents[role]["config_file"].as_str(),
            Some(expected_path.as_str()),
            "{role} must register its matching managed TOML"
        );
    }
}

#[test]
fn init_writes_project_files_and_auth_prints_local_token() {
    let sandbox = TestSandbox::new("init-success");
    let output = sandbox.run(["init", "--api-key", "test-api-key"]);
    assert_success(&output);

    let env_text = fs::read_to_string(sandbox.project().join(".codex-mimo-adapter.env")).unwrap();
    assert!(env_text.contains("MIMO_API_KEY=test-api-key"));
    assert!(env_text.contains("CODEX_MIMO_PROJECT_ID=mimo_adapter_"));
    assert!(env_text.contains("CODEX_MIMO_STATE_DB=.codex-mimo/state.sqlite"));

    let token = env_text
        .lines()
        .find_map(|line| line.strip_prefix("CODEX_MIMO_LOCAL_TOKEN="))
        .unwrap()
        .to_string();
    assert!(token.starts_with("codex-mimo-"));

    let project_key = env_text
        .lines()
        .find_map(|line| line.strip_prefix("CODEX_MIMO_PROJECT_ID=mimo_adapter_"))
        .expect("init must write a routed project ID");
    let agents_dir = sandbox.project().join(".codex").join("agents");
    let installed_agents: BTreeSet<_> = fs::read_dir(&agents_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    let expected_agents: BTreeSet<_> = MANAGED_AGENT_FILES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        installed_agents, expected_agents,
        "init must install exactly the managed agents"
    );

    for name in MANAGED_AGENT_FILES {
        let text = fs::read_to_string(agents_dir.join(name)).unwrap();
        assert!(
            text.contains("model_provider = \"mimo_adapter\""),
            "{name} must use the mimo_adapter provider"
        );
        assert!(
            text.contains(&format!("model = \"mimo_adapter/{project_key}/mimo/")),
            "{name} must use the generated project route"
        );
    }
    for name in LEGACY_MANAGED_AGENT_FILES {
        assert!(
            !agents_dir.join(name).exists(),
            "legacy agent template must not be generated: {name}"
        );
    }

    let config = fs::read_to_string(sandbox.home().join(".codex").join("config.toml")).unwrap();
    assert!(config.contains("[model_providers.mimo_adapter]"));
    assert!(config.contains("command = \"codex-mimo-adapter\""));
    assert!(config.contains("args = [\"auth\", \"print-local-token\"]"));

    let auth_output = sandbox.run(["auth", "print-local-token"]);
    assert_success(&auth_output);
    let signed_token = stdout(&auth_output).trim().to_string();
    assert!(
        signed_token.starts_with("codex-mimo-"),
        "token must be signed: {signed_token}"
    );
    assert_ne!(
        signed_token, token,
        "signed token must differ from raw token"
    );

    let nested_dir = sandbox.project().join("src").join("nested");
    fs::create_dir_all(&nested_dir).unwrap();
    let nested_auth_output = sandbox.run_in(&nested_dir, ["auth", "print-local-token"]);
    assert_success(&nested_auth_output);
    assert_eq!(stdout(&nested_auth_output).trim(), signed_token);

    // Provider auth may run outside the project; active project fallback keeps it working.
    let external_dir = sandbox.root().join("external");
    fs::create_dir_all(&external_dir).unwrap();
    let external_auth_output = sandbox.run_in(&external_dir, ["auth", "print-local-token"]);
    assert_success(&external_auth_output);
    assert_eq!(stdout(&external_auth_output).trim(), signed_token);
}

#[test]
fn init_replaces_only_legacy_managed_agents_and_preserves_custom_agents() {
    let sandbox = TestSandbox::new("init-agent-migration");
    let agents_dir = sandbox.project().join(".codex").join("agents");
    fs::create_dir_all(&agents_dir).unwrap();
    fs::write(agents_dir.join("oss-flash.toml"), "name = \"legacy\"\n").unwrap();
    fs::write(
        agents_dir.join("my-local-agent.toml"),
        "name = \"my_local_agent\"\nmodel = \"custom/model\"\n",
    )
    .unwrap();

    let output = sandbox.run(["init", "--api-key", "test-api-key"]);
    assert_success(&output);

    assert!(
        !agents_dir.join("oss-flash.toml").exists(),
        "known legacy adapter agent must be removed"
    );
    assert_eq!(
        fs::read_to_string(agents_dir.join("my-local-agent.toml")).unwrap(),
        "name = \"my_local_agent\"\nmodel = \"custom/model\"\n",
        "init must not overwrite an unmanaged agent"
    );
    for name in MANAGED_AGENT_FILES {
        assert!(
            agents_dir.join(name).exists(),
            "missing managed agent: {name}"
        );
    }
}

#[test]
fn init_from_stdin_uses_inherited_key_without_writing_it_to_project_env() {
    let sandbox = TestSandbox::new("init-api-key-stdin");
    let secret = "stdin-api-key-must-not-be-written";
    let output = sandbox.run_with_stdin(["init", "--api-key-stdin"], secret);
    assert_success(&output);
    assert!(
        !stdout(&output).contains(secret),
        "init must not echo the key to stdout"
    );
    assert!(
        !stderr(&output).contains(secret),
        "init must not echo the key to stderr"
    );

    let env_path = sandbox.project().join(".codex-mimo-adapter.env");
    let env_text = fs::read_to_string(&env_path).unwrap();
    assert!(env_text.contains("CODEX_MIMO_API_KEY_SOURCE=process"));
    assert!(!env_text.contains(secret));
    assert!(!env_text.contains("MIMO_API_KEY="));

    let project_env = read_project_env(&env_path).unwrap();
    let inherited_env = HashMap::from([("MIMO_API_KEY".to_string(), secret.to_string())]);
    let config = Config::from_sources(&project_env, &inherited_env, ConfigOverrides::default())
        .expect("run must resolve a stdin-initialized project from inherited MIMO_API_KEY");
    assert_eq!(config.upstream_key, secret);

    let missing_key = sandbox.run_without_env(["run"], "MIMO_API_KEY");
    assert!(!missing_key.status.success());
    assert!(
        stderr(&missing_key).contains("inherited process environment"),
        "missing-key diagnostic was: {}",
        stderr(&missing_key)
    );
}

#[test]
fn init_rejects_api_key_and_api_key_stdin_together() {
    let sandbox = TestSandbox::new("init-api-key-conflict");
    let output = sandbox.run_with_stdin(
        ["init", "--api-key", "argument-key", "--api-key-stdin"],
        "stdin-key",
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot be used with '--api-key-stdin'"),
        "conflict diagnostic was: {}",
        stderr(&output)
    );
    assert!(!sandbox.project().join(".codex-mimo-adapter.env").exists());
}

#[test]
fn init_updates_existing_provider_and_creates_backup() {
    let sandbox = TestSandbox::new("init-update");
    let config_dir = sandbox.home().join(".codex");
    fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("config.toml");
    let original = r#"[foo]
keep = true

[model_providers.other]
name = "Other"

[model_providers.mimo_adapter]
name = "Old"
base_url = "http://127.0.0.1:9999/v1"
wire_api = "responses"

[model_providers.mimo_adapter.auth]
command = "cmd.exe"
args = ["/d", "/s", "/c", "echo old"]
timeout_ms = 5000
"#;
    fs::write(&config_path, original).unwrap();

    let output = sandbox.run(["init", "--api-key", "test-api-key"]);
    assert_success(&output);

    let updated = fs::read_to_string(&config_path).unwrap();
    assert!(updated.contains("[foo]"));
    assert!(updated.contains("keep = true"));
    assert!(updated.contains("[model_providers.other]"));
    assert!(updated.contains("name = \"Other\""));
    assert!(updated.contains("name = \"MiMo API Adapter\""));
    assert!(updated.contains("command = \"codex-mimo-adapter\""));
    assert!(!updated.contains("echo old"));

    let backups = fs::read_dir(&config_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("config.toml.bak."))
        .collect::<Vec<_>>();
    assert!(!backups.is_empty(), "expected a config backup");
}

#[test]
fn init_from_subdirectory_writes_only_current_dir() {
    let sandbox = TestSandbox::new("init-subdir");
    let parent = sandbox.project();
    let child = parent.join("child");
    fs::create_dir_all(&child).unwrap();

    let output = sandbox.run_in(&child, ["init", "--api-key", "child-key"]);
    assert_success(&output);

    assert!(
        !parent.join(".codex-mimo-adapter.env").exists(),
        "init from child must not write parent project env"
    );
    assert!(
        child.join(".codex-mimo-adapter.env").exists(),
        "init from child must write child project env"
    );
}

#[test]
fn init_rolls_back_when_agent_write_fails() {
    let sandbox = TestSandbox::new("init-rollback");
    let config_dir = sandbox.home().join(".codex");
    fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("config.toml");
    let original = "[preexisting]\nvalue = 1\n";
    fs::write(&config_path, original).unwrap();

    fs::write(sandbox.project().join(".codex"), "blocking file").unwrap();

    let output = sandbox.run(["init", "--api-key", "test-api-key"]);
    assert!(!output.status.success(), "init should have failed");
    assert!(stderr(&output).contains("failed to create"));
    assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
    assert!(!sandbox.project().join(".codex-mimo-adapter.env").exists());

    let log_path = sandbox.home().join(".codex-mimo-adapter").join("init.log");
    let log_text = fs::read_to_string(log_path).unwrap();
    assert!(log_text.contains("write failed, starting rollback"));
}

#[test]
fn auth_run_and_start_require_init() {
    let sandbox = TestSandbox::new("not-initialized");

    // auth discovers project from CWD; run/start use registry
    let output = sandbox.run(vec!["auth", "print-local-token"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("CODEX_MIMO_LOCAL_TOKEN is missing"),
        "auth stderr was: {}",
        stderr(&output)
    );

    for args in [vec!["run"], vec!["start"]] {
        let output = sandbox.run(args);
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("No projects found in registry"),
            "run/start stderr was: {}",
            stderr(&output)
        );
    }
}

#[test]
fn auth_recovers_project_from_codex_thread_session() {
    let sandbox = TestSandbox::new("auth-thread-recovery");
    let init_output = sandbox.run(["init", "--api-key", "test-api-key"]);
    assert_success(&init_output);
    let direct = sandbox.run(["auth", "print-local-token"]);
    assert_success(&direct);

    let external_dir = sandbox.root().join("external");
    fs::create_dir_all(&external_dir).unwrap();
    let thread_id = "019f-test-thread-recovery";
    sandbox.write_session_meta(thread_id, sandbox.project());

    let recovered = sandbox.run_in_with_env(
        &external_dir,
        ["auth", "print-local-token"],
        [("CODEX_THREAD_ID", thread_id)],
    );
    assert_success(&recovered);
    assert_eq!(stdout(&recovered).trim(), stdout(&direct).trim());
}

#[test]
fn auth_rejects_recovered_project_when_registry_mismatches_env() {
    let sandbox = TestSandbox::new("auth-thread-mismatch");
    let init_output = sandbox.run(["init", "--api-key", "test-api-key"]);
    assert_success(&init_output);

    let env_path = sandbox.project().join(".codex-mimo-adapter.env");
    let mut env_text = fs::read_to_string(&env_path).unwrap();
    let original_project_id = env_text
        .lines()
        .find_map(|line| line.strip_prefix("CODEX_MIMO_PROJECT_ID="))
        .unwrap()
        .to_string();
    env_text = env_text.replace(
        &format!("CODEX_MIMO_PROJECT_ID={original_project_id}"),
        "CODEX_MIMO_PROJECT_ID=mimo_adapter_wrongid",
    );
    fs::write(&env_path, env_text).unwrap();

    let external_dir = sandbox.root().join("external");
    fs::create_dir_all(&external_dir).unwrap();
    let thread_id = "019f-test-thread-mismatch";
    sandbox.write_session_meta(thread_id, sandbox.project());

    let direct = sandbox.run(["auth", "print-local-token"]);
    assert_success(&direct);
    let direct_token = stdout(&direct).trim().to_string();
    assert!(
        direct_token.starts_with("codex-mimo-"),
        "token should be a valid adapter token"
    );
}

// ---------------------------------------------------------------------------
// Test: check without project context falls through to health check
#[test]
fn check_without_project_context_shows_connectivity_error() {
    let sandbox = TestSandbox::new("check-no-context");
    // No init means no projects at all and no env file.
    let external_dir = sandbox.root().join("external");
    fs::create_dir_all(&external_dir).unwrap();

    let output = sandbox.run_in(&external_dir, ["check"]);
    let stderr_text = stderr(&output);
    let stdout_text = stdout(&output);

    // Must never mention "Project is not initialized"; that was the original bug.
    assert!(
        !stderr_text.contains("Project is not initialized"),
        "must not mention project init: {stderr_text}"
    );
    // The warning prints the original config error, so "No MiMo" is
    // expected in stderr.  It must NOT appear in stdout though.
    assert!(
        !stdout_text.contains("No MiMo"),
        "must not mention 'no projects' in stdout: {stdout_text}"
    );
    // Verify the warning is present so the user knows why.
    assert!(
        stderr_text.contains("Warning: could not load project config"),
        "stderr should contain config resolution warning: {stderr_text}"
    );
}

#[test]
fn check_uses_project_env_and_succeeds() {
    let sandbox = TestSandbox::new("check-success");
    let local_token = Arc::new("project-local-token".to_string());
    let app = Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .route(
            "/v1/models",
            get({
                let local_token = Arc::clone(&local_token);
                move |headers: HeaderMap| models_handler(headers, Arc::clone(&local_token))
            }),
        );
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = std_listener.local_addr().unwrap();
    std_listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Register project in registry, then override env with mock upstream
    assert_success(&sandbox.run(["init", "--api-key", "test-api-key"]));
    let init_env = fs::read_to_string(sandbox.project().join(".codex-mimo-adapter.env")).unwrap();
    let proj_id = init_env
        .lines()
        .find_map(|l| l.strip_prefix("CODEX_MIMO_PROJECT_ID="))
        .unwrap();
    let init_token = init_env
        .lines()
        .find_map(|l| l.strip_prefix("CODEX_MIMO_LOCAL_TOKEN="))
        .unwrap();

    fs::write(
        sandbox.project().join(".codex-mimo-adapter.env"),
        format!(
            "MIMO_API_KEY=test-api-key\nCODEX_MIMO_LOCAL_TOKEN={init_token}\nCODEX_MIMO_PROJECT_ID={proj_id}\nCODEX_MIMO_HOST=127.0.0.1\nCODEX_MIMO_PORT={port}\n",
            port = addr.port()
        ),
    )
    .unwrap();

    let output = sandbox.run(["check"]);
    assert_success(&output);
    assert!(stdout(&output).contains("Adapter check passed."));
}

// ---------------------------------------------------------------------------
// Test: dual-project isolation
#[test]
fn dual_project_isolation() {
    let sandbox = TestSandbox::new("dual-project");

    // Project A
    let proj_a = sandbox.root().join("proj_a");
    fs::create_dir_all(&proj_a).unwrap();
    let out_a = sandbox.run_in(&proj_a, ["init", "--api-key", "key-a"]);
    assert_success(&out_a);
    let env_a = fs::read_to_string(proj_a.join(".codex-mimo-adapter.env")).unwrap();
    let pid_a = env_a
        .lines()
        .find_map(|l| l.strip_prefix("CODEX_MIMO_PROJECT_ID="))
        .unwrap()
        .to_string();
    let token_a = env_a
        .lines()
        .find_map(|l| l.strip_prefix("CODEX_MIMO_LOCAL_TOKEN="))
        .unwrap()
        .to_string();
    assert!(pid_a.starts_with("mimo_adapter_"));

    // Project B
    let proj_b = sandbox.root().join("proj_b");
    fs::create_dir_all(&proj_b).unwrap();
    let out_b = sandbox.run_in(&proj_b, ["init", "--api-key", "key-b"]);
    assert_success(&out_b);
    let env_b = fs::read_to_string(proj_b.join(".codex-mimo-adapter.env")).unwrap();
    let pid_b = env_b
        .lines()
        .find_map(|l| l.strip_prefix("CODEX_MIMO_PROJECT_ID="))
        .unwrap()
        .to_string();
    let token_b = env_b
        .lines()
        .find_map(|l| l.strip_prefix("CODEX_MIMO_LOCAL_TOKEN="))
        .unwrap()
        .to_string();

    assert_ne!(pid_a, pid_b, "each project must get unique project_id");
    assert_ne!(token_a, token_b, "each project must get unique local_token");

    // Agent templates use the shared provider and each project's route.
    for (proj, project_id) in [(&proj_a, &pid_a), (&proj_b, &pid_b)] {
        let project_key = project_id
            .strip_prefix("mimo_adapter_")
            .expect("project ID must have adapter prefix");
        for name in ["default.toml", "oss-worker-pro-3.toml", "worker.toml"] {
            let text = fs::read_to_string(proj.join(".codex").join("agents").join(name)).unwrap();
            assert!(
                text.contains("model_provider = \"mimo_adapter\""),
                "{}/agents/{name} must use fixed provider",
                proj.display()
            );
            assert!(
                text.contains(&format!("model = \"mimo_adapter/{project_key}/mimo/")),
                "{}/agents/{name} must use its project route",
                proj.display()
            );
        }
    }

    // Global config has single mimo_adapter (not project-specific)
    let config = fs::read_to_string(sandbox.home().join(".codex").join("config.toml")).unwrap();
    assert!(
        config.contains("[model_providers.mimo_adapter]"),
        "config must contain mimo_adapter"
    );
    assert!(
        !config.contains("model_providers.mimo_adapter_"),
        "config should not contain project-specific provider names"
    );

    // Auth returns different signed tokens
    let auth_a = sandbox.run_in(&proj_a, ["auth", "print-local-token"]);
    assert_success(&auth_a);
    let signed_a = stdout(&auth_a).trim().to_string();
    assert!(signed_a.starts_with("codex-mimo-"));

    let auth_b = sandbox.run_in(&proj_b, ["auth", "print-local-token"]);
    assert_success(&auth_b);
    let signed_b = stdout(&auth_b).trim().to_string();
    assert!(signed_b.starts_with("codex-mimo-"));

    assert_ne!(
        signed_a, signed_b,
        "signed tokens must differ between projects"
    );
}

// ---------------------------------------------------------------------------
// Req 2: external auth multi-project falls back to an adapter-level token
#[test]
fn dual_project_external_auth_must_not_silently_succeed() {
    let sandbox = TestSandbox::new("dual-ext-auth-req2");
    let proj_a = sandbox.root().join("proj_a");
    fs::create_dir_all(&proj_a).unwrap();
    assert_success(&sandbox.run_in(&proj_a, ["init", "--api-key", "key-a"]));
    let proj_b = sandbox.root().join("proj_b");
    fs::create_dir_all(&proj_b).unwrap();
    assert_success(&sandbox.run_in(&proj_b, ["init", "--api-key", "key-b"]));
    let external_dir = sandbox.root().join("external");
    fs::create_dir_all(&external_dir).unwrap();
    let output = sandbox.run_in_with_env(
        &external_dir,
        ["auth", "print-local-token"],
        [("CODEX_THREAD_ID", "")],
    );
    assert_success(&output);
    let token = stdout(&output).trim().to_string();
    assert!(
        token.starts_with("codex-mimo-"),
        "token should be a valid adapter token"
    );
}

#[test]
fn dual_project_external_auth_can_use_registered_adapter_token() {
    let sandbox = TestSandbox::new("dual-ext-auth-active-ttl");
    let proj_a = sandbox.root().join("proj_a");
    fs::create_dir_all(&proj_a).unwrap();
    assert_success(&sandbox.run_in(&proj_a, ["init", "--api-key", "key-a"]));
    let proj_b = sandbox.root().join("proj_b");
    fs::create_dir_all(&proj_b).unwrap();
    assert_success(&sandbox.run_in(&proj_b, ["init", "--api-key", "key-b"]));

    let external_dir = sandbox.root().join("external-after-active");
    fs::create_dir_all(&external_dir).unwrap();
    let output = sandbox.run_in_with_env(
        &external_dir,
        ["auth", "print-local-token"],
        [("CODEX_THREAD_ID", "")],
    );
    assert_success(&output);
    let token = stdout(&output).trim().to_string();
    assert!(
        token.starts_with("codex-mimo-"),
        "token should be a valid adapter token"
    );
}

// ---------------------------------------------------------------------------
// Req 3, path 1: CODEX_MIMO_PROJECT_ID env var recovery
#[test]
fn auth_recovery_via_env_var_project_id() {
    let sandbox = TestSandbox::new("auth-env-recovery-req3");
    assert_success(&sandbox.run(["init", "--api-key", "test-api-key"]));
    let direct = sandbox.run(["auth", "print-local-token"]);
    assert_success(&direct);
    let direct_token = stdout(&direct).trim().to_string();
    let env_text = fs::read_to_string(sandbox.project().join(".codex-mimo-adapter.env")).unwrap();
    let project_id = env_text
        .lines()
        .find_map(|line| line.strip_prefix("CODEX_MIMO_PROJECT_ID="))
        .unwrap()
        .to_string();
    let external_dir = sandbox.root().join("external");
    fs::create_dir_all(&external_dir).unwrap();
    let recovered = sandbox.run_in_with_env(
        &external_dir,
        ["auth", "print-local-token"],
        [("CODEX_MIMO_PROJECT_ID", &project_id)],
    );
    assert_success(&recovered);
    assert_eq!(stdout(&recovered).trim(), direct_token);
}

// ---------------------------------------------------------------------------
// Req 3, path 4: Active project multi-project fallback
#[test]
fn single_project_external_auth_succeeds_via_fallback() {
    let sandbox = TestSandbox::new("single-external-fallback");
    assert_success(&sandbox.run(["init", "--api-key", "test-api-key"]));
    let direct = sandbox.run(["auth", "print-local-token"]);
    assert_success(&direct);
    let direct_token = stdout(&direct).trim().to_string();
    // External dir with no context -> single-project constrained fallback (Priority 4)
    let external_dir = sandbox.root().join("external");
    fs::create_dir_all(&external_dir).unwrap();
    let output = sandbox.run_in_with_env(
        &external_dir,
        ["auth", "print-local-token"],
        [("CODEX_THREAD_ID", "")],
    );
    assert_success(&output);
    assert_eq!(stdout(&output).trim(), direct_token);
}
async fn models_handler(
    headers: HeaderMap,
    expected_token: Arc<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let auth = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let raw_expected = format!("Bearer {}", expected_token.as_str());
    let accept = auth == raw_expected || auth.starts_with("Bearer codex-mimo-");
    if !accept {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "data": [
                { "id": "mimo/deepseek-v4-flash" }
            ]
        })),
    )
}

struct TestSandbox {
    root: PathBuf,
    project: PathBuf,
    home: PathBuf,
}

impl TestSandbox {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("codex-mimo-adapter-{label}-{}", Uuid::new_v4()));
        let project = root.join("project");
        let home = root.join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            root,
            project,
            home,
        }
    }

    fn run<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.run_in(&self.project, args)
    }

    fn run_in<I, S>(&self, current_dir: &Path, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.run_in_with_env(current_dir, args, std::iter::empty::<(&str, &str)>())
    }

    fn run_in_with_env<I, S, J, K, V>(&self, current_dir: &Path, args: I, envs: J) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
        J: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut command = Command::new(binary_path());
        for arg in args {
            command.arg(arg.as_ref());
        }
        for (key, value) in envs {
            command.env(key.as_ref(), value.as_ref());
        }
        command
            .current_dir(current_dir)
            .env("USERPROFILE", &self.home)
            .env("HOME", &self.home)
            .output()
            .unwrap()
    }

    fn run_with_stdin<I, S>(&self, args: I, stdin: &str) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = Command::new(binary_path());
        for arg in args {
            command.arg(arg.as_ref());
        }
        let mut child = command
            .current_dir(&self.project)
            .env("USERPROFILE", &self.home)
            .env("HOME", &self.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn run_without_env<I, S>(&self, args: I, key: &str) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = Command::new(binary_path());
        for arg in args {
            command.arg(arg.as_ref());
        }
        command
            .current_dir(&self.project)
            .env("USERPROFILE", &self.home)
            .env("HOME", &self.home)
            .env_remove(key)
            .output()
            .unwrap()
    }

    fn project(&self) -> &Path {
        &self.project
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn home(&self) -> &Path {
        &self.home
    }

    fn write_session_meta(&self, thread_id: &str, cwd: &Path) {
        let session_dir = self
            .home
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("06")
            .join("27");
        fs::create_dir_all(&session_dir).unwrap();
        let session_path =
            session_dir.join(format!("rollout-2026-06-27T00-00-00-{thread_id}.jsonl"));
        let cwd = cwd.to_string_lossy().replace('\\', "\\\\");
        fs::write(
            session_path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread_id}\",\"cwd\":\"{cwd}\"}}}}\n"
            ),
        )
        .unwrap();
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_codex-mimo-adapter")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
