/* =============================================================================
 * File          : outbound.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One small HTTP client for the features that talk to a third party —
 *   certificate automation and the dynamic DNS updater. The request/response
 *   pair is deliberately plain, and the `Transport` trait is the seam that
 *   lets both features be tested against a scripted peer instead of the
 *   network.
 *
 * Dependencies:
 *   - reqwest (>= 0.13)
 *
 * Usage:
 *   `ReqwestTransport::new()?` in production; a scripted `Transport` in tests.
 * =============================================================================
 */

//! The outbound HTTP seam shared by certificate automation and dynamic DNS.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use crate::acme::AcmeError;

/// Anonymous future: keeps `Transport` `dyn`-safe without an `async_trait`
/// dependency.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// HTTP method of an outbound request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Head,
    Post,
    Put,
    Delete,
}

/// One request to the certificate authority or a DNS provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    /// `None` for GET/HEAD.
    pub body: Option<String>,
    pub content_type: &'static str,
    /// `Authorization: Bearer …`, used by DNS providers (never by the ACME
    /// server, which authenticates the JWS body itself).
    pub bearer: Option<String>,
}

impl HttpRequest {
    pub fn get(url: &str) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.to_string(),
            body: None,
            content_type: "application/json",
            bearer: None,
        }
    }

    pub fn head(url: &str) -> Self {
        Self {
            method: HttpMethod::Head,
            url: url.to_string(),
            body: None,
            content_type: "application/json",
            bearer: None,
        }
    }

    pub fn post(url: &str, body: String, content_type: &'static str) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.to_string(),
            body: Some(body),
            content_type,
            bearer: None,
        }
    }

    pub fn put(url: &str, body: String, content_type: &'static str) -> Self {
        Self {
            method: HttpMethod::Put,
            url: url.to_string(),
            body: Some(body),
            content_type,
            bearer: None,
        }
    }

    pub fn delete(url: &str) -> Self {
        Self {
            method: HttpMethod::Delete,
            url: url.to_string(),
            body: None,
            content_type: "application/json",
            bearer: None,
        }
    }

    /// Attach a provider API token.
    pub fn with_bearer(mut self, token: &str) -> Self {
        self.bearer = Some(token.to_string());
        self
    }
}

/// A response, with lower-cased header names so `Replay-Nonce` and
/// `replay-nonce` are the same lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    pub headers: BTreeMap<String, String>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The one seam between this module and the network. Tests implement it with a
/// scripted CA; production uses [`ReqwestTransport`].
pub trait Transport: Send + Sync {
    fn send<'a>(&'a self, request: HttpRequest) -> BoxFuture<'a, Result<HttpResponse, AcmeError>>;
}

/// `reqwest`-backed transport, with the same deadline as every other outbound
/// call HomeVault makes.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, AcmeError> {
        reqwest::Client::builder()
            .timeout(crate::acme::HTTP_TIMEOUT)
            .user_agent(concat!("HomeVault/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
            .map_err(|e| AcmeError::new(format!("cannot build the HTTP client: {e}"), true))
    }
}

impl Transport for ReqwestTransport {
    fn send<'a>(&'a self, request: HttpRequest) -> BoxFuture<'a, Result<HttpResponse, AcmeError>> {
        Box::pin(async move {
            let mut builder = match request.method {
                HttpMethod::Get => self.client.get(&request.url),
                HttpMethod::Head => self.client.head(&request.url),
                HttpMethod::Post => self.client.post(&request.url),
                HttpMethod::Put => self.client.put(&request.url),
                HttpMethod::Delete => self.client.delete(&request.url),
            };
            if let Some(token) = request.bearer {
                builder = builder.bearer_auth(token);
            }
            if let Some(body) = request.body {
                builder = builder
                    .header(reqwest::header::CONTENT_TYPE, request.content_type)
                    .body(body);
            }
            let response = builder.send().await.map_err(|e| {
                AcmeError::new(
                    format!(
                        "cannot reach {}: {e}",
                        host_of(&request.url).unwrap_or("the certificate authority")
                    ),
                    true,
                )
            })?;
            let status = response.status().as_u16();
            let mut headers = BTreeMap::new();
            for (name, value) in response.headers().iter() {
                if let Ok(value) = value.to_str() {
                    headers.insert(name.as_str().to_ascii_lowercase(), value.to_string());
                }
            }
            let body = response.text().await.unwrap_or_default();
            Ok(HttpResponse {
                status,
                body,
                headers,
            })
        })
    }
}

/// Host part of a URL, for error messages that name a destination without
/// echoing a query string or a token.
pub fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty())
}
