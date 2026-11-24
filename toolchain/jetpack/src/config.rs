//! Registry configuration, including project/user `.npmrc`, scoped registries,
//! and origin/path-bound bearer tokens. This module never logs credentials.
//!
//! Registry priority runs from `--registry` to `NPM_CONFIG_REGISTRY`, project
//! `.npmrc`, user `.npmrc`, and npmjs. Scoped registries are read from `.npmrc`.
//! Tokens are sent only to a matching URL origin and path prefix.
//! `cafile=` replaces the trusted roots with the PEM certificates of that file.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};
use url::{Host, Url};

/// Registry used when no override is provided.
pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org/";

/// Non-secret registry routing snapshot stored in the lockfile.
/// Changing this configuration requires re-resolution instead of replaying
/// package downloads from an old registry, even when versions match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySources {
    /// Registry for ordinary packages and scopes without an override.
    pub default: String,
    /// Mapping from npm scope (`@company`) to registry base URL.
    #[serde(default)]
    pub scopes: BTreeMap<String, String>,
}

impl Default for RegistrySources {
    fn default() -> Self {
        Self {
            default: DEFAULT_REGISTRY.to_string(),
            scopes: BTreeMap::new(),
        }
    }
}

impl RegistrySources {
    /// Verify all registry base URLs and scope identifiers before use.
    pub fn validate(&self) -> Result<()> {
        if normalize_registry(&self.default)? != self.default {
            bail!("non-canonical default registry URL");
        }
        for (scope, url) in &self.scopes {
            validate_scope(scope)?;
            if normalize_registry(url)? != *url {
                bail!("non-canonical registry for {scope}");
            }
        }
        Ok(())
    }

    /// Registry selected for a package according to its npm scope.
    pub fn for_package(&self, package: &str) -> &str {
        package
            .split_once('/')
            .and_then(|(scope, _)| self.scopes.get(scope))
            .map(String::as_str)
            .unwrap_or(&self.default)
    }
}

#[derive(Clone)]
struct ScopedToken {
    https: Url,
    http: Url,
    value: String,
}

/// Registry routes and credentials. Credential values are deliberately omitted
/// from `Debug` and never serialized into the lockfile.
#[derive(Clone)]
pub struct RegistryConfig {
    sources: RegistrySources,
    tokens: Vec<ScopedToken>,
    /// PEM certificates from `cafile=`: the only roots trusted for registry TLS when set.
    ca: Option<Vec<u8>>,
}

impl std::fmt::Debug for RegistryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryConfig")
            .field("sources", &self.sources)
            .field("credential_scopes", &self.tokens.len())
            .field("custom_ca", &self.ca.is_some())
            .finish()
    }
}

impl RegistryConfig {
    /// Construct a single-registry configuration, without credentials.
    /// Loopback HTTP is supported for development registries (e.g. Verdaccio).
    ///
    /// ```
    /// use jetpack::config::RegistryConfig;
    /// let config = RegistryConfig::single("http://127.0.0.1:4873")?
    ///     .with_scope("@company", "http://localhost:4874/npm")?;
    /// assert_eq!(config.sources().for_package("@company/pkg"),
    ///     "http://localhost:4874/npm/");
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn single(url: &str) -> Result<Self> {
        Ok(Self {
            sources: RegistrySources {
                default: normalize_registry(url)?,
                scopes: BTreeMap::new(),
            },
            tokens: Vec::new(),
            ca: None,
        })
    }

    /// Read `~/.npmrc`, then `./.npmrc`, then the registry environment override
    /// and finally the explicit CLI override. `NPM_CONFIG_USERCONFIG` selects
    /// an alternative user file. No global npmrc or proxy options are processed.
    pub fn load(project: &Path, cli_registry: Option<&str>) -> Result<Self> {
        let user_file = env::var_os("NPM_CONFIG_USERCONFIG")
            .or_else(|| env::var_os("npm_config_userconfig"))
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|path| path.join(".npmrc")));
        let env_registry = env::var("NPM_CONFIG_REGISTRY")
            .ok()
            .or_else(|| env::var("npm_config_registry").ok());
        Self::from_files(
            project,
            user_file.as_deref(),
            env_registry.as_deref(),
            cli_registry,
        )
    }

    /// Deterministic file-based configuration loader, also useful for tests.
    /// User `.npmrc` may expand `${ENV_NAME}`; project `.npmrc` deliberately
    /// rejects such expansion to avoid leaking CI secrets from untrusted repos.
    pub fn from_files(
        project: &Path,
        user_file: Option<&Path>,
        env_registry: Option<&str>,
        cli_registry: Option<&str>,
    ) -> Result<Self> {
        let mut config = Self::single(DEFAULT_REGISTRY)?;
        if let Some(file) = user_file {
            config.apply_file(file, true)?;
        }
        config.apply_file(&project.join(".npmrc"), false)?;
        if let Some(url) = env_registry {
            config.sources.default = normalize_registry(url)?;
        }
        if let Some(url) = cli_registry {
            config.sources.default = normalize_registry(url)?;
        }
        config.sources.validate()?;
        Ok(config)
    }

    /// Explicitly route one npm scope to a separate registry.
    pub fn with_scope(mut self, scope: &str, url: &str) -> Result<Self> {
        validate_scope(scope)?;
        self.sources
            .scopes
            .insert(scope.to_owned(), normalize_registry(url)?);
        Ok(self)
    }

    /// Configure one bearer token for the URL prefix expressed in npmrc form,
    /// such as `//127.0.0.1:4873/:_authToken`. Credentials are never sent to
    /// another origin or outside the path prefix, including for tarball downloads.
    /// HTTP is selected only for an explicitly configured loopback registry.
    pub fn with_token(mut self, npmrc_key: &str, token: &str) -> Result<Self> {
        self.set_token(npmrc_key, token)?;
        Ok(self)
    }

    /// Non-secret registry configuration suitable for a reproducible lockfile.
    pub fn sources(&self) -> &RegistrySources {
        &self.sources
    }

    /// PEM roots from `cafile=`, replacing the default roots for registry TLS.
    pub fn ca(&self) -> Option<&[u8]> {
        self.ca.as_deref()
    }

    /// Return a token only if a request URL matches its origin and path.
    /// The longest matching path wins so registry-specific credentials take
    /// priority over host-wide credentials.
    pub fn token_for<'a>(&'a self, url: &Url) -> Option<&'a str> {
        self.tokens
            .iter()
            .filter_map(|token| {
                // A scheme-less npmrc key binds to one origin. HTTPS takes
                // precedence if both schemes are configured for the key.
                let scope = if !self.configured_origin(&token.https)
                    && self.configured_origin(&token.http)
                {
                    &token.http
                } else {
                    &token.https
                };
                (scope.origin() == url.origin() && url.path().starts_with(scope.path()))
                    .then_some((scope.path().len(), token.value.as_str()))
            })
            .max_by_key(|(length, _)| *length)
            .map(|(_, token)| token)
    }

    fn configured_origin(&self, url: &Url) -> bool {
        std::iter::once(&self.sources.default)
            .chain(self.sources.scopes.values())
            .filter_map(|value| Url::parse(value).ok())
            .any(|registry| registry.origin() == url.origin())
    }

    pub(crate) fn check_destination(&self, url: &Url) -> Result<()> {
        let local = match url.host() {
            Some(Host::Domain(host)) => {
                host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
                    || host
                        .trim_end_matches('.')
                        .to_ascii_lowercase()
                        .ends_with(".localhost")
            }
            Some(Host::Ipv4(ip)) => is_local_ipv4(ip),
            Some(Host::Ipv6(ip)) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_unicast_link_local()
                    || ip.is_unique_local()
                    || ip.is_multicast()
                    || ip.to_ipv4_mapped().is_some_and(is_local_ipv4)
            }
            None => false,
        };
        if local && !self.configured_origin(url) {
            bail!("local registry request requires an explicitly configured registry origin");
        }
        Ok(())
    }

    fn apply_file(&mut self, path: &Path, allow_env: bool) -> Result<()> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((raw_key, raw_value)) = line.split_once('=') else {
                continue;
            };
            let key = raw_key.trim();
            let mut value = raw_value.trim().to_owned();
            if !(key == "registry"
                || key == "cafile"
                || key.ends_with(":registry")
                || key.ends_with(":_authToken"))
            {
                continue;
            }
            if value.contains("${") {
                if !allow_env {
                    bail!(
                        "environment interpolation is forbidden in project .npmrc at {}:{}",
                        path.display(),
                        index + 1
                    );
                }
                value = expand_env(&value)
                    .with_context(|| format!("{}:{}", path.display(), index + 1))?;
            }
            if key == "registry" {
                self.sources.default = normalize_registry(&value)?;
            } else if key == "cafile" {
                // A relative path is relative to the .npmrc that names it.
                let file = path.parent().unwrap_or(Path::new(".")).join(&value);
                let pem = fs::read(&file).with_context(|| {
                    format!(
                        "cannot read cafile {} named at {}:{}",
                        file.display(),
                        path.display(),
                        index + 1
                    )
                })?;
                if !pem.windows(27).any(|w| w == b"-----BEGIN CERTIFICATE-----") {
                    bail!("cafile {} contains no PEM certificate", file.display());
                }
                self.ca = Some(pem);
            } else if let Some(scope) = key.strip_suffix(":registry") {
                validate_scope(scope)?;
                self.sources
                    .scopes
                    .insert(scope.into(), normalize_registry(&value)?);
            } else {
                self.set_token(key, &value)?;
            }
        }
        Ok(())
    }

    fn set_token(&mut self, key: &str, token: &str) -> Result<()> {
        let prefix = key
            .strip_suffix(":_authToken")
            .context("expected a registry-scoped _authToken key")?;
        if !prefix.starts_with("//")
            || token.is_empty()
            || token.chars().any(|c| c == '\n' || c == '\r')
        {
            bail!("invalid registry-scoped token configuration");
        }
        let parse = |scheme| -> Result<Url> {
            let mut url = Url::parse(&format!("{scheme}:{prefix}"))
                .context("invalid registry token scope")?;
            if !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                bail!("invalid registry token scope");
            }
            if !url.path().ends_with('/') {
                url.set_path(&format!("{}/", url.path()));
            }
            Ok(url)
        };
        let https = parse("https")?;
        let http = parse("http")?;
        self.tokens
            .retain(|existing| existing.https != https || existing.http != http);
        self.tokens.push(ScopedToken {
            https,
            http,
            value: token.into(),
        });
        Ok(())
    }
}

fn is_local_ipv4(ip: std::net::Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || matches!(ip.octets(), [100, 64..=127, _, _] | [0, _, _, _])
}

/// Validate and canonicalize an npm-compatible registry base URL.
/// HTTPS is required for remote hosts; plain HTTP is allowed only on loopback.
pub fn normalize_registry(raw: &str) -> Result<String> {
    crate::registry::validate_url(raw)?;
    let mut url = Url::parse(raw).context("invalid registry URL")?;
    if url.query().is_some() || url.fragment().is_some() {
        bail!("registry URL must not include query or fragment");
    }
    let path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url.to_string())
}

fn validate_scope(scope: &str) -> Result<()> {
    if !scope.starts_with('@')
        || scope.len() < 2
        || !scope[1..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        bail!("invalid npm scope: {scope}");
    }
    Ok(())
}

fn expand_env(raw: &str) -> Result<String> {
    let mut output = String::new();
    let mut remaining = raw;
    while let Some(start) = remaining.find("${") {
        output.push_str(&remaining[..start]);
        let rest = &remaining[start + 2..];
        let end = rest
            .find('}')
            .context("unterminated environment variable in .npmrc")?;
        let name = &rest[..end];
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            bail!("invalid environment variable name in .npmrc");
        }
        output.push_str(
            &env::var(name).with_context(|| format!("missing environment variable {name}"))?,
        );
        remaining = &rest[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_project_and_user_registry_with_expected_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.npmrc");
        fs::write(
            &user,
            "registry=https://user.example/\n@private:registry=https://private.example/npm\n",
        )
        .unwrap();
        fs::write(
            dir.path().join(".npmrc"),
            "registry=http://127.0.0.1:4873\n@private:registry=http://localhost:4874/packages\n",
        )
        .unwrap();
        let c = RegistryConfig::from_files(dir.path(), Some(&user), None, None).unwrap();
        assert_eq!(c.sources.default, "http://127.0.0.1:4873/");
        assert_eq!(
            c.sources.for_package("@private/thing"),
            "http://localhost:4874/packages/"
        );
        assert_eq!(c.sources.for_package("ordinary"), "http://127.0.0.1:4873/");
        let c = RegistryConfig::from_files(
            dir.path(),
            Some(&user),
            Some("https://env.example"),
            Some("https://cli.example/"),
        )
        .unwrap();
        assert_eq!(c.sources.default, "https://cli.example/");
        assert_eq!(
            c.sources.for_package("@private/a"),
            "http://localhost:4874/packages/"
        );
    }

    #[test]
    fn secrets_are_bound_to_host_port_and_path() {
        let config = RegistryConfig::single("http://127.0.0.1:4873")
            .unwrap()
            .with_token("//127.0.0.1:4873/:_authToken", "host-token")
            .unwrap()
            .with_token("//127.0.0.1:4873/private/:_authToken", "path-token")
            .unwrap();
        let token = |url: &str| config.token_for(&Url::parse(url).unwrap());
        assert_eq!(
            token("http://127.0.0.1:4873/private/pkg"),
            Some("path-token")
        );
        assert_eq!(
            token("http://127.0.0.1:4873/other/pkg.tgz"),
            Some("host-token")
        );
        assert_eq!(token("http://127.0.0.1:4874/private/pkg"), None);
        assert_eq!(token("https://127.0.0.1:4873/private/pkg"), None);
        assert_eq!(token("http://localhost:4873/private/pkg"), None);
        assert_eq!(
            token("http://127.0.0.1:4873/private-evil/pkg"),
            Some("host-token")
        );
        assert!(!format!("{config:?}").contains("host-token"));
    }

    #[test]
    fn project_tokens_override_user_tokens_for_same_origin() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.npmrc");
        let key = "//localhost:4873/:_authToken";
        fs::write(&user, format!("{key}=user-secret\n")).unwrap();
        fs::write(
            dir.path().join(".npmrc"),
            format!("registry=http://localhost:4873/\n{key}=project-secret\n"),
        )
        .unwrap();
        let config = RegistryConfig::from_files(dir.path(), Some(&user), None, None).unwrap();
        let url = Url::parse("http://localhost:4873/packages/a.tgz").unwrap();
        assert_eq!(config.token_for(&url), Some("project-secret"));
    }

    #[test]
    fn rejects_untrusted_project_env_secret_expansion() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".npmrc"),
            "//bad.example/:_authToken=${CI_SECRET}\n",
        )
        .unwrap();
        assert!(RegistryConfig::from_files(dir.path(), None, None, None).is_err());
    }

    #[test]
    fn rejects_invalid_registry_origins_and_credential_scopes() {
        assert!(RegistryConfig::single("http://example.com").is_err());
        assert!(RegistryConfig::single("http://localhost:4873").is_ok());
        assert!(RegistryConfig::single("http://127.0.0.1:4873@evil.example").is_err());
        assert!(RegistryConfig::single("https://example.com?access_token=secret").is_err());
        assert!(RegistryConfig::single("https://example.com")
            .unwrap()
            .with_token("//user@evil.example/:_authToken", "a")
            .is_err());
    }

    #[test]
    fn cafile_is_read_relative_to_its_npmrc() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("corp.pem"),
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        fs::write(dir.path().join(".npmrc"), "cafile=corp.pem\n").unwrap();
        let config = RegistryConfig::from_files(dir.path(), None, None, None).unwrap();
        assert!(config
            .ca()
            .unwrap()
            .starts_with(b"-----BEGIN CERTIFICATE-----"));
        fs::write(dir.path().join(".npmrc"), "cafile=missing.pem\n").unwrap();
        assert!(RegistryConfig::from_files(dir.path(), None, None, None).is_err());
    }
}
