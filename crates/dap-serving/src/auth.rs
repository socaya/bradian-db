//! Temporary fail-closed bootstrap administrator gate.
//! This is not a substitute for production identity, RBAC or TLS.
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use crate::state::SharedState;

pub async fn require_bootstrap_admin(
    State(state): State<SharedState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = request.headers().get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty());
    let principal = token
        .and_then(|value| state.api_tokens.resolve(value))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !matches!(principal.role, dap_security::Role::Administrator) {
        return Err(StatusCode::FORBIDDEN);
    }
    request.extensions_mut().insert(principal);
    Ok(next.run(request).await)
}
