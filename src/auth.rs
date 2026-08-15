use std::sync::Arc;

use axum::{
    extract::Request,
    extract::State,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use gcp_auth::provider;

use crate::config::Config;

pub struct ApiKey {
    expected: String,
}

impl ApiKey {
    pub async fn load(config: &Config) -> anyhow::Result<Arc<Self>> {
        let secret = fetch_secret(&config.gcp_project, &config.secret_name).await?;
        Ok(Arc::new(Self { expected: secret }))
    }

    pub fn is_valid(&self, auth_header: Option<&str>) -> bool {
        let Some(token) = auth_header.and_then(|h| h.strip_prefix("Bearer ")) else {
            return false;
        };
        constant_time_eq(token.as_bytes(), self.expected.as_bytes())
    }
}

pub async fn require_api_key(
    State(key): State<Arc<ApiKey>>,
    request: Request,
    next: Next,
) -> Response {
    let auth = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if key.is_valid(auth) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

async fn fetch_secret(project: &str, name: &str) -> anyhow::Result<String> {
    let token = provider()
        .await?
        .token(&["https://www.googleapis.com/auth/cloud-platform"])
        .await?;
    let token = token.as_str().to_owned();
    let url = format!(
        "https://secretmanager.googleapis.com/v1/projects/{project}/secrets/{name}/versions/latest:access"
    );
    let encoded = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let response: serde_json::Value = ureq::get(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .call()?
            .into_json()?;
        Ok(response["payload"]["data"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("secret has no payload"))?
            .to_owned())
    })
    .await??;
    Ok(String::from_utf8(BASE64_STANDARD.decode(encoded)?)?)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_bearer_matches() {
        let key = ApiKey {
            expected: "secret".to_string(),
        };
        assert!(key.is_valid(Some("Bearer secret")));
    }

    #[test]
    fn wrong_or_missing_token_rejected() {
        let key = ApiKey {
            expected: "secret".to_string(),
        };
        assert!(!key.is_valid(Some("Bearer wrong")));
        assert!(!key.is_valid(None));
        assert!(!key.is_valid(Some("secret")));
    }
}
