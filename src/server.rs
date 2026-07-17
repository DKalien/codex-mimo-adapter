use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::KeepAlive;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::codex_chat_history::{ensure_no_duplicate_call_outputs, validate_requested_call_ids};
use crate::config::{Config, ConfigOverrides};
use crate::conversion::{
    build_chat_payload, build_response, function_output_call_ids, StreamAssembler,
};
use crate::media_guard::{
    find_unsupported_multimodal_input, is_multimodal_unsupported_error,
    unsupported_multimodal_error_message,
};
use crate::project::{
    current_environment, project_id_from_key, project_key_from_id, read_project_env,
    registry_dir_path, validate_adapter_token, ProjectRegistry, GLOBAL_PROJECT_ID,
    PROJECT_ENV_FILENAME,
};
use crate::state::{now_ts, StateStore};
use crate::upstream::{
    extract_error_message, parse_chat_sse_bytes, sse_data_from_block, sse_event_from_block,
    MimoClient, UpstreamError,
};

const DOWNSTREAM_SSE_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

fn downstream_sse_keep_alive(interval: Duration) -> KeepAlive {
    KeepAlive::new().interval(interval).text("keep-alive")
}

#[derive(Clone)]
pub struct ProjectRuntime {
    pub config: Config,
    pub client: MimoClient,
    pub state: StateStore,
}

#[derive(Clone)]
pub struct AppState {
    pub projects: Arc<RwLock<HashMap<String, ProjectRuntime>>>,
    pub capacity: Arc<Semaphore>,
    pub config_overrides: ConfigOverrides,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/models", get(models))
        .route("/v1/responses", post(responses))
        .route("/responses", post(responses))
        .route("/admin/refresh", post(admin_refresh))
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({"status":"ok"}))
}

async fn models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let runtimes = {
        let projects = state.projects.read().unwrap();
        if let Err(response) = authorize_adapter(&projects, &headers) {
            return *response;
        }
        projects
            .iter()
            .map(|(project_id, runtime)| (project_id.clone(), runtime.clone()))
            .collect::<Vec<_>>()
    };

    let mut data = Vec::new();
    for (project_id, runtime) in runtimes {
        match runtime.client.models().await {
            Ok(raw) => {
                let rows = raw
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let project_key = project_key_from_id(&project_id);
                data.extend(rows.into_iter().filter_map(|row| {
                    row.get("id").and_then(Value::as_str).map(|id| {
                        json!({
                            "id":format!("mimo_adapter/{project_key}/mimo/{id}"),
                            "object":"model",
                            "owned_by":"mimo"
                        })
                    })
                }));
            }
            Err(error) => return upstream_error(error),
        }
    }
    Json(json!({"object":"list","data":data})).into_response()
}

async fn responses(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    let body: Value = match serde_json::from_str(&body) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "request body must be an object",
            )
        }
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &error.to_string(),
            )
        }
    };
    let model_alias = match body.get("model").and_then(Value::as_str) {
        Some(model) => model.to_string(),
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "model is required",
            )
        }
    };
    let (project_id, model_upstream) = match parse_routed_model(&model_alias) {
        Ok(route) => route,
        Err(message) => {
            return responses_failed_response_with_status(
                StatusCode::BAD_REQUEST,
                &body,
                &model_alias,
                "invalid_model",
                message,
            )
        }
    };

    let runtime = {
        let projects = state.projects.read().unwrap();
        if let Err(response) = authorize_adapter(&projects, &headers) {
            return *response;
        }
        match project_runtime(&projects, &project_id) {
            Some(r) => r.clone(),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "project_not_found",
                    &format!(
                        "Project route {project_id} is not loaded. Run 'codex-mimo-adapter init' for that project and call POST /admin/refresh."
                    ),
                ).into_response();
            }
        }
    };
    if body.to_string().len() > runtime.config.max_request_bytes {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request_too_large",
            "Invalid request size",
        );
    }
    if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        stream_response(
            runtime,
            state.capacity.clone(),
            body,
            model_alias,
            model_upstream,
        )
        .await
    } else {
        complete_response(
            runtime,
            state.capacity.clone(),
            body,
            model_alias,
            model_upstream,
        )
        .await
    }
}

async fn complete_response(
    runtime: ProjectRuntime,
    capacity: Arc<Semaphore>,
    body: Value,
    model_alias: String,
    model_upstream: String,
) -> Response {
    let previous = match previous_response(&runtime.state, &body) {
        Ok(previous) => previous,
        Err(message) => {
            return responses_failed_response(&body, &model_alias, "invalid_tool_history", &message)
        }
    };
    let (payload, messages, _reverse, tool_ctx) =
        match build_chat_payload(&body, &model_upstream, previous.as_ref(), json!({})) {
            Ok(value) => value,
            Err(error) => {
                let message = error.to_string();
                if is_history_error_message(&message) {
                    return responses_failed_response(
                        &body,
                        &model_alias,
                        "invalid_tool_history",
                        &message,
                    );
                }
                return error_response(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
            }
        };
    if let Some(message) = find_unsupported_multimodal_input(&model_upstream, &payload) {
        return responses_failed_response(
            &body,
            &model_alias,
            "unsupported_multimodal_input",
            &message,
        );
    }
    let _permit = match capacity.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                "adapter concurrency limit reached",
            )
        }
    };
    let upstream = runtime.client.chat(payload).await;
    drop(_permit);
    let upstream = match upstream {
        Ok(value) => value,
        Err(error) => {
            let status = upstream_http_status(&error);
            let message = error.to_string();
            if is_multimodal_unsupported_error(&message) {
                return responses_failed_response(
                    &body,
                    &model_alias,
                    "unsupported_multimodal_input",
                    unsupported_multimodal_error_message(),
                );
            }
            return responses_failed_response_with_status(
                status,
                &body,
                &model_alias,
                "upstream_error",
                &message,
            );
        }
    };
    match build_response(
        &body,
        &upstream,
        &model_alias,
        &model_upstream,
        &messages,
        &tool_ctx,
        |item| runtime.state.put(&item),
    ) {
        Ok(response) => Json(response).into_response(),
        Err(error) => responses_failed_response_with_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            &body,
            &model_alias,
            "internal_error",
            &error.to_string(),
        ),
    }
}

async fn stream_response(
    runtime: ProjectRuntime,
    capacity: Arc<Semaphore>,
    body: Value,
    model_alias: String,
    model_upstream: String,
) -> Response {
    let previous = match previous_response(&runtime.state, &body) {
        Ok(previous) => previous,
        Err(message) => {
            return early_stream_failed_response(
                body,
                model_alias,
                "invalid_tool_history",
                &message,
            )
        }
    };
    let (payload, messages, _reverse, tool_ctx) =
        match build_chat_payload(&body, &model_upstream, previous.as_ref(), json!({})) {
            Ok(value) => value,
            Err(error) => {
                let message = error.to_string();
                if is_history_error_message(&message) {
                    return early_stream_failed_response(
                        body,
                        model_alias,
                        "invalid_tool_history",
                        &message,
                    );
                }
                return error_response(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
            }
        };

    if let Some(message) = find_unsupported_multimodal_input(&model_upstream, &payload) {
        return stream_failed_response(
            runtime,
            body,
            model_alias,
            model_upstream,
            messages,
            tool_ctx,
            "unsupported_multimodal_input",
            &message,
        );
    }

    let (tx, rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<axum::response::sse::Event, Infallible>>();
    let runtime_for_task = runtime.clone();
    tokio::spawn(async move {
        let emit_tx = tx.clone();
        let mut assembler = StreamAssembler::new(
            body.clone(),
            model_alias.clone(),
            model_upstream.clone(),
            messages,
            tool_ctx,
            Box::new(move |item| runtime_for_task.state.put(&item)),
            Box::new(move |event, data| {
                let sse = axum::response::sse::Event::default()
                    .event(event)
                    .data(data.to_string());
                let _ = emit_tx.send(Ok(sse));
                Ok(())
            }),
        );
        if let Err(error) = assembler.start() {
            tracing::error!(error = %error, "failed to emit initial stream lifecycle events");

            let response =
                responses_failed_value(&body, &model_alias, "internal_error", &error.to_string());
            let event = json!({"type":"response.failed","response":response});
            let _ = tx.send(Ok(axum::response::sse::Event::default()
                .event("response.failed")
                .data(event.to_string())));
            let _ = tx.send(Ok(axum::response::sse::Event::default().data("[DONE]")));

            return;
        }
        let _permit = match capacity.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                if let Err(error) =
                    assembler.fail("rate_limit_error", "adapter concurrency limit reached")
                {
                    tracing::error!(error = %error, "failed to emit rate-limit stream failure");
                }
                let _ = tx.send(Ok(axum::response::sse::Event::default().data("[DONE]")));
                return;
            }
        };
        let upstream = runtime_for_task.client.chat_stream(payload).await;

        match upstream {
            Ok(mut stream) => {
                let mut buffer = String::new();
                let mut utf8_remainder: Vec<u8> = Vec::new();
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(bytes) => {
                            for block in
                                parse_chat_sse_bytes(&mut buffer, &mut utf8_remainder, &bytes)
                            {
                                let event_name = sse_event_from_block(&block);
                                let data = sse_data_from_block(&block);
                                if event_name.as_deref() == Some("error") {
                                    let message = data
                                        .as_deref()
                                        .and_then(|data| serde_json::from_str::<Value>(data).ok())
                                        .and_then(|value| extract_error_message(&value))
                                        .or_else(|| {
                                            data.clone().filter(|data| !data.trim().is_empty())
                                        })
                                        .unwrap_or_else(|| "upstream stream error".to_string());
                                    let kind = upstream_stream_error_type(&message);
                                    let display = upstream_stream_error_message(&message);
                                    if let Err(error) = assembler.fail(kind, &display) {
                                        tracing::error!(
                                            error = %error,
                                            "failed to emit upstream event:error stream failure"
                                        );
                                    }
                                    let _ = tx.send(Ok(
                                        axum::response::sse::Event::default().data("[DONE]")
                                    ));
                                    return;
                                }
                                if let Some(data) = data {
                                    if data.trim() == "[DONE]" {
                                        if let Err(error) = assembler.finalize() {
                                            tracing::error!(
                                                error = %error,
                                                "failed to finalize stream after upstream [DONE]"
                                            );
                                            let _ = assembler
                                                .fail("internal_error", &error.to_string());
                                        }
                                        let _ = tx
                                            .send(Ok(axum::response::sse::Event::default()
                                                .data("[DONE]")));
                                        return;
                                    }
                                    match serde_json::from_str::<Value>(&data) {
                                        Ok(value) => {
                                            let is_error = value.get("error").is_some()
                                                || value.get("base_resp").is_some();
                                            if is_error {
                                                let message = extract_error_message(&value)
                                                    .unwrap_or_else(|| {
                                                        "upstream stream error".to_string()
                                                    });
                                                let kind = upstream_stream_error_type(&message);
                                                let display =
                                                    upstream_stream_error_message(&message);
                                                if let Err(error) = assembler.fail(kind, &display) {
                                                    tracing::error!(
                                                        error = %error,
                                                        "failed to emit upstream JSON error stream failure"
                                                    );
                                                }
                                                let _ = tx
                                                    .send(Ok(axum::response::sse::Event::default(
                                                    )
                                                    .data("[DONE]")));
                                                return;
                                            }
                                            if let Err(error) = assembler.accept(&value) {
                                                tracing::error!(
                                                    error = %error,
                                                    "failed to process upstream stream chunk"
                                                );
                                                let _ = assembler
                                                    .fail("internal_error", &error.to_string());
                                                let _ = tx
                                                    .send(Ok(axum::response::sse::Event::default(
                                                    )
                                                    .data("[DONE]")));
                                                return;
                                            }
                                        }
                                        Err(error) => {
                                            let message = format!(
                                                "failed to parse upstream SSE data as JSON: {error}"
                                            );
                                            if let Err(error) =
                                                assembler.fail("upstream_error", &message)
                                            {
                                                tracing::error!(
                                                    error = %error,
                                                    "failed to emit upstream parse-error stream failure"
                                                );
                                            }
                                            let _ = tx
                                                .send(Ok(axum::response::sse::Event::default()
                                                    .data("[DONE]")));
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            let message = error.to_string();
                            let kind = upstream_stream_error_type(&message);
                            let display = upstream_stream_error_message(&message);
                            if let Err(error) = assembler.fail(kind, &display) {
                                tracing::error!(
                                    error = %error,
                                    "failed to emit network stream failure"
                                );
                            }
                            let _ =
                                tx.send(Ok(axum::response::sse::Event::default().data("[DONE]")));
                            return;
                        }
                    }
                }
                if assembler.has_finish_reason() {
                    if let Err(error) = assembler.finalize() {
                        tracing::error!(
                            error = %error,
                            "failed to finalize stream with finish_reason"
                        );
                        let _ = assembler.fail("internal_error", &error.to_string());
                    }
                } else if assembler.has_substantive_output() {
                    assembler.mark_truncated_as_length();
                    if let Err(error) = assembler.finalize() {
                        tracing::error!(
                            error = %error,
                            "failed to finalize truncated stream"
                        );
                        let _ = assembler.fail("internal_error", &error.to_string());
                    }
                } else {
                    if let Err(error) = assembler.fail(
                        "stream_truncated",
                        "Upstream stream ended before sending finish_reason",
                    ) {
                        tracing::error!(
                            error = %error,
                            "failed to emit stream-truncated failure"
                        );
                    }
                }
                let _ = tx.send(Ok(axum::response::sse::Event::default().data("[DONE]")));
            }
            Err(error) => {
                let message = error.to_string();
                let kind = upstream_stream_error_type(&message);
                let display = upstream_stream_error_message(&message);
                if let Err(error) = assembler.fail(kind, &display) {
                    tracing::error!(
                        error = %error,
                        "failed to emit initial upstream stream failure"
                    );
                }
                let _ = tx.send(Ok(axum::response::sse::Event::default().data("[DONE]")));
            }
        }
    });

    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    Sse::new(stream)
        .keep_alive(downstream_sse_keep_alive(
            DOWNSTREAM_SSE_KEEP_ALIVE_INTERVAL,
        ))
        .into_response()
}

fn previous_response(
    state_store: &StateStore,
    body: &Value,
) -> Result<Option<crate::state::StoredResponse>, String> {
    let ids = function_output_call_ids(body).map_err(|e| e.to_string())?;
    ensure_no_duplicate_call_outputs(&ids).map_err(|e| e.to_string())?;

    if let Some(previous_id) = body
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    {
        let previous = state_store.get(previous_id).map_err(|e| e.to_string())?;
        if let Some(previous) = previous.as_ref() {
            validate_requested_call_ids(previous, &ids).map_err(|e| e.to_string())?;
        } else if !ids.is_empty() {
            return Err(format!(
                "tool output has no matching stored response: {previous_id}"
            ));
        }
        return Ok(previous);
    }

    if ids.is_empty() {
        return Ok(None);
    }

    let previous = state_store
        .find_by_call_ids(&ids)
        .map_err(|e| e.to_string())?;
    let Some(previous) = previous else {
        return Err(format!(
            "unknown or ambiguous tool call id(s): {}",
            ids.join(", ")
        ));
    };
    validate_requested_call_ids(&previous, &ids).map_err(|e| e.to_string())?;
    Ok(Some(previous))
}

fn adapter_bearer_token(headers: &HeaderMap) -> Result<&str, Box<Response>> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            Box::new(error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Missing Authorization header. Provide a valid Bearer token.",
            ))
        })?;
    auth_header.strip_prefix("Bearer ").ok_or_else(|| {
        Box::new(error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid Authorization format. Expected 'Bearer <token>'.",
        ))
    })
}

fn project_runtime<'a>(
    projects: &'a HashMap<String, ProjectRuntime>,
    project_id: &str,
) -> Option<&'a ProjectRuntime> {
    projects
        .get(project_id)
        .or_else(|| projects.get(project_key_from_id(project_id)))
        .or_else(|| {
            let canonical_id = project_id_from_key(project_id);
            projects.get(&canonical_id)
        })
}

fn runtime_accepts_token(runtime: &ProjectRuntime, raw_token: &str) -> bool {
    runtime
        .config
        .local_token
        .as_ref()
        .is_some_and(|local_token| {
            !local_token.is_empty() && validate_adapter_token(raw_token, local_token)
        })
}

fn authorize_adapter(
    projects: &HashMap<String, ProjectRuntime>,
    headers: &HeaderMap,
) -> Result<(), Box<Response>> {
    let raw_token = adapter_bearer_token(headers)?;
    if projects
        .values()
        .any(|runtime| runtime_accepts_token(runtime, raw_token))
    {
        return Ok(());
    }
    Err(Box::new(error_response(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Invalid or expired adapter token. Run 'codex-mimo-adapter auth print-local-token' again.",
    )))
}

async fn admin_refresh(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Auth: accept any valid adapter bearer token.
    let auth_ok = {
        let projects = state.projects.read().unwrap();
        authorize_adapter(&projects, &headers).is_ok()
    };
    if !auth_ok {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid or missing token. Provide a valid adapter bearer token.",
        );
    }
    let reg_dir = match registry_dir_path() {
        Ok(d) => d,
        Err(e) => return Json(json!({"status":"error","message": e.to_string()})).into_response(),
    };
    let registry = ProjectRegistry::load(&reg_dir);

    let mut projects = state.projects.write().unwrap();
    let mut added = Vec::new();
    let mut already_loaded = Vec::new();
    let mut failed = Vec::new();

    // Remove projects that no longer exist in the registry.
    let mut removed = Vec::new();
    let to_remove: Vec<String> = projects
        .keys()
        .filter(|id| {
            id.as_str() != GLOBAL_PROJECT_ID && !registry.projects.contains_key(id.as_str())
        })
        .cloned()
        .collect();
    for id in &to_remove {
        projects.remove(id);
        removed.push(id.clone());
    }

    for (project_id, entry) in &registry.projects {
        if projects.contains_key(project_id) {
            already_loaded.push(project_id.clone());
            continue;
        }

        let root = PathBuf::from(&entry.root);
        let env_path = root.join(PROJECT_ENV_FILENAME);
        if !env_path.exists() {
            tracing::warn!("refresh: project {project_id} missing env file, skipping");
            failed.push(json!({"project_id": project_id, "reason": "missing env file"}));
            continue;
        }

        let project_env = match read_project_env(&env_path) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("refresh: cannot read env for {project_id}: {e}");
                failed.push(
                    json!({"project_id": project_id, "reason": format!("read env failed: {e}")}),
                );
                continue;
            }
        };
        // The adapter process owns MIMO_API_KEY when init used --api-key-stdin.
        // Keep the local token project-scoped so unrelated process environments
        // cannot change adapter authentication during a refresh.
        let mut env = current_environment();
        env.remove("CODEX_MIMO_LOCAL_TOKEN");
        let config = match Config::from_sources(&project_env, &env, state.config_overrides.clone())
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("refresh: bad config for {project_id}: {e}");
                failed
                    .push(json!({"project_id": project_id, "reason": format!("bad config: {e}")}));
                continue;
            }
        };
        let state_db_path = root.join(&config.state_db);
        let store = match StateStore::new(
            state_db_path.display().to_string(),
            config.state_ttl_seconds,
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("refresh: cannot create state for {project_id}: {e}");
                failed.push(
                    json!({"project_id": project_id, "reason": format!("state init failed: {e}")}),
                );
                continue;
            }
        };
        let client = match MimoClient::new(
            &config.upstream_base,
            &config.upstream_key,
            config.timeout_seconds,
        ) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("refresh: cannot create client for {project_id}: {e}");
                failed.push(
                    json!({"project_id": project_id, "reason": format!("client init failed: {e}")}),
                );
                continue;
            }
        };
        projects.insert(
            project_id.clone(),
            ProjectRuntime {
                config,
                client,
                state: store,
            },
        );
        added.push(project_id.clone());
    }

    Json(json!({"status":"ok","added":added,"already_loaded":already_loaded,"removed":removed,"failed":failed})).into_response()
}

fn parse_routed_model(model: &str) -> Result<(String, String), &'static str> {
    let Some(rest) = model.strip_prefix("mimo_adapter/") else {
        return Err("model must use mimo_adapter/<project_key>/<real_model>. Run 'codex-mimo-adapter init' to refresh agent templates.");
    };
    let Some((project_key, real_model)) = rest.split_once('/') else {
        return Err("model must include both project_key and real_model");
    };
    if project_key.is_empty() {
        return Err("project_key is empty");
    }
    if real_model.is_empty() {
        return Err("real model id is empty");
    }
    let Some(upstream_model) = real_model.strip_prefix("mimo/") else {
        return Err("real_model must use the mimo/ prefix");
    };
    if upstream_model.is_empty() {
        return Err("real model id is empty");
    }
    let project_id = if project_key == GLOBAL_PROJECT_ID {
        GLOBAL_PROJECT_ID.to_string()
    } else {
        project_id_from_key(project_key)
    };
    Ok((project_id, upstream_model.to_string()))
}

fn error_response(status: StatusCode, kind: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error":{"type":kind,"message":message}})),
    )
        .into_response()
}

fn responses_failed_response(body: &Value, model: &str, kind: &str, message: &str) -> Response {
    Json(responses_failed_value(body, model, kind, message)).into_response()
}

fn responses_failed_response_with_status(
    status: StatusCode,
    body: &Value,
    model: &str,
    kind: &str,
    message: &str,
) -> Response {
    (
        status,
        Json(responses_failed_value(body, model, kind, message)),
    )
        .into_response()
}

fn responses_failed_value(body: &Value, model: &str, kind: &str, message: &str) -> Value {
    json!({
        "id": format!("resp_{}", Uuid::new_v4().simple()),
        "object": "response",
        "created_at": now_ts(),
        "status": "failed",
        "error": {
            "type": kind,
            "code": kind,
            "message": message.chars().take(1000).collect::<String>()
        },
        "incomplete_details": null,
        "instructions": body.get("instructions").cloned().unwrap_or(Value::Null),
        "model": model,
        "output": [],
        "parallel_tool_calls": body.get("parallel_tool_calls").and_then(Value::as_bool).unwrap_or(false),
        "previous_response_id": body.get("previous_response_id").cloned().unwrap_or(Value::Null),
        "store": false,
        "usage": empty_response_usage(),
        "metadata": body.get("metadata").cloned().unwrap_or_else(|| json!({}))
    })
}

fn early_stream_failed_response(
    body: Value,
    model_alias: String,
    kind: &'static str,
    message: &str,
) -> Response {
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let created_at = now_ts();
    let shell = json!({"id":response_id,"object":"response","created_at":created_at,"status":"in_progress","error":null,"incomplete_details":null,"instructions":body.get("instructions").cloned().unwrap_or(Value::Null),"model":model_alias,"output":[],"parallel_tool_calls":body.get("parallel_tool_calls").and_then(Value::as_bool).unwrap_or(false),"previous_response_id":body.get("previous_response_id").cloned().unwrap_or(Value::Null),"store":false,"usage":empty_response_usage(),"metadata":body.get("metadata").cloned().unwrap_or_else(|| json!({}))});
    let (tx, rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<axum::response::sse::Event, Infallible>>();
    let _ = tx.send(Ok(axum::response::sse::Event::default()
        .event("response.created")
        .data(
            json!({"type":"response.created","response":shell.clone()}).to_string(),
        )));
    let _ = tx.send(Ok(axum::response::sse::Event::default()
        .event("response.in_progress")
        .data(
            json!({"type":"response.in_progress","response":shell.clone()}).to_string(),
        )));
    let mut failed_response = shell;
    failed_response["status"] = json!("failed");
    failed_response["error"] =
        json!({"type":kind,"code":kind,"message":message.chars().take(1000).collect::<String>()});
    let _ = tx.send(Ok(axum::response::sse::Event::default()
        .event("response.failed")
        .data(
            json!({"type":"response.failed","response":failed_response}).to_string(),
        )));
    let _ = tx.send(Ok(axum::response::sse::Event::default().data("[DONE]")));
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    Sse::new(stream).into_response()
}

fn empty_response_usage() -> Value {
    json!({
        "input_tokens": 0,
        "input_tokens_details": {"cached_tokens": 0},
        "output_tokens": 0,
        "output_tokens_details": {"reasoning_tokens": 0},
        "total_tokens": 0
    })
}

fn stream_failed_response(
    runtime: ProjectRuntime,
    body: Value,
    model_alias: String,
    model_upstream: String,
    messages: Vec<Value>,
    tool_ctx: crate::conversion::tool_context::ToolContext,
    kind: &'static str,
    message: &str,
) -> Response {
    let (tx, rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<axum::response::sse::Event, Infallible>>();
    let emit_tx = tx.clone();
    let runtime_for_emit = runtime.clone();
    let mut assembler = StreamAssembler::new(
        body,
        model_alias,
        model_upstream,
        messages,
        tool_ctx,
        Box::new(move |item| runtime_for_emit.state.put(&item)),
        Box::new(move |event, data| {
            let sse = axum::response::sse::Event::default()
                .event(event)
                .data(data.to_string());
            let _ = emit_tx.send(Ok(sse));
            Ok(())
        }),
    );
    let _ = assembler.start();
    let _ = assembler.fail(kind, message);
    let _ = tx.send(Ok(axum::response::sse::Event::default().data("[DONE]")));
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    Sse::new(stream).into_response()
}

fn is_history_error_message(message: &str) -> bool {
    message.contains("tool output")
        || message.contains("tool call")
        || message.contains("tool history")
        || message.contains("duplicate tool")
        || message.contains("unknown tool")
        || message.contains("invalid tool")
}

fn upstream_stream_error_type(message: &str) -> &'static str {
    if is_multimodal_unsupported_error(message) {
        "unsupported_multimodal_input"
    } else {
        "upstream_error"
    }
}

fn upstream_stream_error_message(message: &str) -> String {
    if is_multimodal_unsupported_error(message) {
        unsupported_multimodal_error_message().to_string()
    } else {
        message.to_string()
    }
}

fn upstream_http_status(error: &UpstreamError) -> StatusCode {
    match error {
        UpstreamError::Http { status, .. } => {
            StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY)
        }
        UpstreamError::Network(_) | UpstreamError::Invalid(_) => StatusCode::BAD_GATEWAY,
    }
}

fn upstream_error(error: UpstreamError) -> Response {
    match error {
        UpstreamError::Http { status, message } => error_response(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            "upstream_error",
            &message,
        ),
        UpstreamError::Network(message) | UpstreamError::Invalid(message) => {
            error_response(StatusCode::BAD_GATEWAY, "upstream_error", &message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_responses_error_has_complete_usage_shape() {
        let response = responses_failed_value(
            &json!({"metadata": {"request": "test"}}),
            "mimo/test-model",
            "upstream_error",
            "failed",
        );

        assert_eq!(
            response["usage"],
            json!({
                "input_tokens": 0,
                "input_tokens_details": {"cached_tokens": 0},
                "output_tokens": 0,
                "output_tokens_details": {"reasoning_tokens": 0},
                "total_tokens": 0
            })
        );
    }

    #[test]
    fn global_route_resolves_without_a_project_registry_id() {
        assert_eq!(
            parse_routed_model("mimo_adapter/global/mimo/mimo-v2.5"),
            Ok(("global".to_string(), "mimo-v2.5".to_string()))
        );
    }

    #[tokio::test]
    async fn downstream_sse_emits_keep_alive_while_response_stream_is_idle() {
        let stream = futures::stream::pending::<
            Result<axum::response::sse::Event, std::convert::Infallible>,
        >();
        let response = Sse::new(stream)
            .keep_alive(downstream_sse_keep_alive(Duration::from_millis(5)))
            .into_response();
        let mut body = response.into_body().into_data_stream();

        let chunk = tokio::time::timeout(Duration::from_millis(100), body.next())
            .await
            .expect("keep-alive should arrive before the test timeout")
            .expect("SSE body should still be open")
            .expect("keep-alive body chunk should be readable");

        assert_eq!(std::str::from_utf8(&chunk).unwrap(), ": keep-alive\n\n");
    }
}
