use std::net::{IpAddr, SocketAddr};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use super::common::{
    decode_text, invalid_input, limit_tokens, parse_input, tool_schema, DEFAULT_MAX_TOKENS,
};

const WEB_RESPONSE_CAP_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WebFetchInput {
    pub url: String,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub raw: bool,
}

#[derive(Clone, Default)]
pub struct WebFetchTool;

#[async_trait::async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch an HTTP(S) URL and return untrusted markdown-like content."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<WebFetchInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        if !ctx.allow_network {
            return Err(ToolError::NetworkDenied(
                "network disabled in tool context".to_string(),
            ));
        }

        let input = parse_input::<WebFetchInput>(self.name(), input)?;
        let url = validate_public_http_url(&input.url).await?;
        let (response, redirects) = fetch_with_redirects(url).await?;
        let final_url = response.url().to_string();

        if response
            .content_length()
            .is_some_and(|length| length > WEB_RESPONSE_CAP_BYTES as u64)
        {
            return Err(ToolError::OutputTooLarge {
                tool_name: self.name().to_string(),
                limit: WEB_RESPONSE_CAP_BYTES,
            });
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|err| invalid_input(self.name(), err.to_string()))?;
        if bytes.len() > WEB_RESPONSE_CAP_BYTES {
            return Err(ToolError::OutputTooLarge {
                tool_name: self.name().to_string(),
                limit: WEB_RESPONSE_CAP_BYTES,
            });
        }

        let text = decode_text(self.name(), &bytes)?;
        let body = if input.raw {
            text
        } else {
            html_to_markdownish(&text)
        };
        let body = limit_tokens(&body, input.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS));
        let redirect_header = if redirects.is_empty() {
            String::new()
        } else {
            format!(" redirects=\"{}\"", redirects.join(" -> "))
        };
        Ok(ToolOutput::Text {
            content: format!(
                "<source id=\"{final_url}\" trust=\"external_untrusted\"{redirect_header}>\n{body}\n</source>"
            ),
        })
    }
}

async fn validate_public_http_url(input: &str) -> Result<Url, ToolError> {
    let url = Url::parse(input).map_err(|err| invalid_input("web_fetch", err.to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(ToolError::NetworkDenied(format!(
                "unsupported scheme: {scheme}"
            )))
        }
    }

    let host = url
        .host_str()
        .ok_or_else(|| invalid_input("web_fetch", "URL must include host"))?;
    if host == "localhost" {
        return Err(ToolError::NetworkDenied("localhost denied".to_string()));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        reject_private_ip(ip)?;
        return Ok(url);
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
    for addr in addrs {
        reject_private_ip(addr.ip())?;
    }
    Ok(url)
}

async fn fetch_with_redirects(mut url: Url) -> Result<(reqwest::Response, Vec<String>), ToolError> {
    let mut redirects = Vec::new();
    for _ in 0..10 {
        let client = pinned_client_for(&url).await?;
        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
        if !response.status().is_redirection() {
            return Ok((response, redirects));
        }

        let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
            return Err(invalid_input(
                "web_fetch",
                "redirect missing Location header",
            ));
        };
        let location = location
            .to_str()
            .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
        let next_url = url
            .join(location)
            .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
        validate_public_http_url(next_url.as_str()).await?;
        redirects.push(format!("{url} -> {next_url}"));
        url = next_url;
    }

    Err(invalid_input("web_fetch", "too many redirects"))
}

async fn pinned_client_for(url: &Url) -> Result<reqwest::Client, ToolError> {
    let url = validate_public_http_url(url.as_str()).await?;
    let host = url
        .host_str()
        .ok_or_else(|| invalid_input("web_fetch", "URL must include host"))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addr = resolve_public_socket_addr(host, port).await?;

    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, addr)
        .build()
        .map_err(|err| invalid_input("web_fetch", err.to_string()))
}

async fn resolve_public_socket_addr(host: &str, port: u16) -> Result<SocketAddr, ToolError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        reject_private_ip(ip)?;
        return Ok(SocketAddr::new(ip, port));
    }

    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
    if let Some(addr) = addrs.next() {
        reject_private_ip(addr.ip())?;
        return Ok(addr);
    }

    Err(ToolError::NetworkDenied(format!(
        "no public addresses resolved for host: {host}"
    )))
}

fn reject_private_ip(ip: IpAddr) -> Result<(), ToolError> {
    let denied = match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.octets()[0] == 0
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.segments()[0] & 0xfe00 == 0xfc00
                || ip.segments()[0] & 0xffc0 == 0xfe80
        }
    };

    if denied {
        return Err(ToolError::NetworkDenied(format!("private IP denied: {ip}")));
    }
    Ok(())
}

fn html_to_markdownish(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                output.push(' ');
            }
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, temp_workspace};
    use super::{html_to_markdownish, validate_public_http_url, WebFetchTool};
    use gestalt_core::{Tool, ToolError};
    use serde_json::json;

    #[tokio::test]
    async fn web_fetch_should_reject_non_http_scheme() {
        let root = temp_workspace("web-scheme");
        let mut ctx = ctx(&root);
        ctx.allow_network = true;
        let result = WebFetchTool::default()
            .execute(json!({"url": "file:///etc/passwd"}), &ctx)
            .await;

        assert!(matches!(result, Err(ToolError::NetworkDenied(_))));
    }

    #[tokio::test]
    async fn web_fetch_should_reject_private_ip() {
        let result = validate_public_http_url("http://127.0.0.1").await;

        assert!(matches!(result, Err(ToolError::NetworkDenied(_))));
    }

    #[test]
    fn html_extraction_should_strip_tags() {
        assert_eq!(
            html_to_markdownish("<h1>Hello</h1><p>world</p>"),
            "Hello world"
        );
    }
}
