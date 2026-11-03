//! Every mint request uses checked, pinned DNS results and bounded responses.
use async_trait::async_trait;
use cdk::nuts::{AuthToken, CurrencyUnit};
use cdk::wallet::{MintConnector, Wallet, WalletConfig, WalletRepository};
use cdk_http_client::{HttpError, RawResponse, Transport};
use reqwest::{Method, Url};
use serde::{de::DeserializeOwned, Serialize};
use std::{net::IpAddr, sync::Arc, time::Duration};

const MAX_RESPONSE: usize = 4 * 1024 * 1024;

pub fn parse_url(raw: &str) -> Result<Url, HttpError> {
    if raw.len() > 3072 {
        return Err(HttpError::Other("mint URL is too long".into()));
    }
    let url = Url::parse(raw).map_err(|e| HttpError::Other(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(HttpError::Other(
            "mint URL must be HTTP(S), without credentials, query or fragment".into(),
        ));
    }
    Ok(url)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_documentation()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && (18..=19).contains(&b))
                && !(a == 192 && b == 0 && c == 0)
                && !(a == 192 && b == 88 && c == 99)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_ip(v4.into());
            }
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

#[derive(Debug, Clone)]
pub struct MintTransport {
    private_origins: Arc<Vec<String>>,
}

impl Default for MintTransport {
    fn default() -> Self {
        let origins = std::env::var("AUDITOR_PRIVATE_MINT_URLS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| parse_url(s.trim()).ok())
            .map(|u| u.origin().ascii_serialization())
            .collect();
        Self {
            private_origins: Arc::new(origins),
        }
    }
}

impl MintTransport {
    async fn request(
        &self,
        method: Method,
        url: Url,
        auth: Option<AuthToken>,
        body: Option<(Vec<u8>, &'static str)>,
    ) -> Result<RawResponse, HttpError> {
        let deadline = super::net::NetConfig::from_env()
            .request_timeout
            .max(Duration::from_secs(1));
        tokio::time::timeout(deadline, async {
            let url = parse_url(url.as_str())?;
            let host = url
                .host_str()
                .ok_or_else(|| HttpError::Other("missing mint host".into()))?;
            let host = host.trim_start_matches('[').trim_end_matches(']');
            let port = url
                .port_or_known_default()
                .ok_or_else(|| HttpError::Other("missing mint port".into()))?;
            let addresses: Vec<_> = tokio::net::lookup_host((host, port))
                .await
                .map_err(|e| HttpError::Connection(e.to_string()))?
                .collect();
            if addresses.is_empty()
                || (!self
                    .private_origins
                    .contains(&url.origin().ascii_serialization())
                    && addresses.iter().any(|a| !public_ip(a.ip())))
            {
                return Err(HttpError::Other(
                    "mint destination is not a public address".into(),
                ));
            }
            // Pin the checked answers: a second DNS lookup would allow rebinding.
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(deadline)
                .resolve_to_addrs(host, &addresses)
                .build()
                .map_err(|e| HttpError::Build(e.to_string()))?;
            let mut request = client.request(method, url);
            if let Some(auth) = auth {
                request = request.header(auth.header_key(), auth.to_string());
            }
            if let Some((bytes, content_type)) = body {
                request = request.header("content-type", content_type).body(bytes);
            }
            let mut response = request.send().await.map_err(http_error)?;
            let status = response.status().as_u16();
            if response
                .content_length()
                .is_some_and(|n| n > MAX_RESPONSE as u64)
            {
                return Err(HttpError::Other("mint response exceeds 4 MiB".into()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(http_error)? {
                if chunk.len() > MAX_RESPONSE.saturating_sub(bytes.len()) {
                    return Err(HttpError::Other("mint response exceeds 4 MiB".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(RawResponse::new(status, bytes))
        })
        .await
        .map_err(|_| HttpError::Timeout)?
    }
}

fn http_error(e: reqwest::Error) -> HttpError {
    if e.is_timeout() {
        HttpError::Timeout
    } else {
        HttpError::Connection(e.to_string())
    }
}

#[async_trait]
impl Transport for MintTransport {
    fn with_proxy(&mut self, _: Url, _: Option<&str>, _: bool) -> Result<(), HttpError> {
        Err(HttpError::Other(
            "mint proxies would bypass destination validation".into(),
        ))
    }
    async fn ws_connect(
        &self,
        _: &str,
        _: &[(&str, &str)],
    ) -> Result<
        (
            cdk_http_client::ws::WsSender,
            cdk_http_client::ws::WsReceiver,
        ),
        cdk_http_client::ws::WsError,
    > {
        Err(cdk_http_client::ws::WsError::NotSupported(
            "mint WebSockets are disabled; use HTTP polling".into(),
        ))
    }
    async fn http_get<R: DeserializeOwned>(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<R, HttpError> {
        self.http_get_raw(url, auth).await?.json_or_status_error()
    }
    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.request(Method::GET, url, auth, None).await
    }
    async fn http_post<P: Serialize + Send + Sync, R: DeserializeOwned>(
        &self,
        url: Url,
        auth: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError> {
        self.request(
            Method::POST,
            url,
            auth,
            Some((serde_json::to_vec(payload)?, "application/json")),
        )
        .await?
        .json_or_status_error()
    }
    async fn http_post_form_raw<P: Serialize + Send + Sync>(
        &self,
        url: Url,
        auth: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError> {
        let body = serde_urlencoded::to_string(payload)
            .map_err(|e| HttpError::Serialization(e.to_string()))?;
        self.request(
            Method::POST,
            url,
            auth,
            Some((body.into_bytes(), "application/x-www-form-urlencoded")),
        )
        .await
    }
}

pub fn connector(mint: cdk::mint_url::MintUrl) -> Arc<dyn MintConnector + Send + Sync> {
    Arc::new(cdk::wallet::BaseHttpClient::with_transport(
        mint,
        MintTransport::default(),
        None,
    ))
}

pub fn wallet_config(mint: cdk::mint_url::MintUrl) -> WalletConfig {
    WalletConfig::new()
        .with_mint_connector(connector(mint.clone()))
        .with_auth_connector(Arc::new(cdk::wallet::BaseAuthHttpClient::with_transport(
            mint,
            MintTransport::default(),
            None,
        )))
}

pub async fn wallet(repository: &WalletRepository, raw: &str) -> Result<Wallet, cdk::Error> {
    parse_url(raw).map_err(|e| cdk::Error::Custom(e.to_string()))?;
    let mint: cdk::mint_url::MintUrl = raw.parse()?;
    repository
        .get_or_create_wallet(mint.clone(), CurrencyUnit::Sat, Some(wallet_config(mint)))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_private_and_special_destinations() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "198.18.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "2002:7f00:1::1",
            "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()));
        }
        for url in [
            "https://bad host/<img>",
            "file:///etc/passwd",
            "http://user:pass@example.com",
            "https://example.com/#x",
        ] {
            assert!(parse_url(url).is_err());
        }
    }
    #[tokio::test]
    async fn private_request_is_rejected_before_connecting() {
        let transport = MintTransport {
            private_origins: Arc::new(vec![]),
        };
        let err = transport
            .http_get_raw(Url::parse("http://127.0.0.1:9/v1/info").unwrap(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a public address"));
    }
}
