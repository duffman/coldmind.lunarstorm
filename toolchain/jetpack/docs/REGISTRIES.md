# Custom and local npm registries

`jetpack` can resolve and install npm registry packages from a self-hosted
npm-compatible service (such as Verdaccio) without changing its dependency
resolver or running Node during installation.

## Configure a local registry

Start your registry separately, then in the project containing `package.json`:

```sh
jetbunny install --registry http://127.0.0.1:4873
```

Or create `.npmrc` in that directory:

```ini
registry=http://127.0.0.1:4873/
@company:registry=http://127.0.0.1:4873/
//127.0.0.1:4873/:_authToken=local-token
```

Packages in the `@company` scope use that registry, including nested and peer
dependencies. Other packages use the default registry.

`--registry` overrides only the default registry; it does not erase separately
configured `@scope:registry` mappings.

## Configuration precedence

For the default registry, from highest to lowest priority:

1. `jetbunny install --registry URL`
2. `NPM_CONFIG_REGISTRY` / `npm_config_registry`
3. Project `./.npmrc`
4. User `~/.npmrc` (or `NPM_CONFIG_USERCONFIG`)
5. `https://registry.npmjs.org/`

Scope routing and bearer-token entries are loaded from user `.npmrc` first,
then overridden by the project `.npmrc`. The implementation intentionally
parses `registry`, `@scope:registry`, `//host/path/:_authToken` and `cafile`.
Other npmrc keys are ignored. A relative `cafile` path is resolved against the
`.npmrc` file that names it. Its PEM certificates replace the default trust roots.

## Credentials and security

A token can be scoped to an origin and path. Example for a registry mounted at
`/npm/`:

```ini
@team:registry=https://packages.example.org/npm/
//packages.example.org/npm/:_authToken=${NPM_TOKEN}
```

**Place secrets referenced with `${...}` in your user `.npmrc`**, not a
repository-controlled project `.npmrc`. Environment expansion in project
`.npmrc` is rejected to prevent untrusted repositories from directing secret
tokens to attacker-controlled servers. Project `.npmrc` may contain literal
scoped tokens, but these should not be committed to version control.

Credentials are sent as `Authorization: Bearer` only when the request's scheme,
host, effective port and path prefix match the selected token origin. Scheme-less
npmrc token keys select HTTPS by default. HTTP is selected only when that loopback
origin is explicitly configured as a registry and the HTTPS origin is absent.
The longest matching path wins.

Up to five redirects are followed, and every hop is checked again. A token is
selected for each hop's destination. HTTP is permitted only for `localhost`,
`127.0.0.1` and `[::1]`. Requests to literal local/private/link-local address hosts
and localhost names require an explicitly configured registry origin, including
HTTPS requests. A public registry cannot redirect to an unconfigured local origin.
These checks inspect the URL host. DNS and network egress policy belong to the
embedder or deployment.

Request debug output and URL diagnostics redact bearer tokens, URL credentials
and query strings. A failed speculative tarball download is retried with the larger
body budget only for a typed `BodyTooLarge` error. Authentication, timeout and other
transport failures retain their error.

Registry metadata may return an absolute tarball URL or a relative path. A
relative path is resolved against the configured registry base URL, allowing
simple private registries with a mount prefix.

## Lockfile behavior

`rustnpm-lock.json` records the default registry and scope routes,
never credentials. When these routes differ from current configuration:

- `jetbunny install` regenerates a new dependency graph and lockfile.
- `jetbunny add` / `remove` resolve without reusing pins from an old registry.
- `jetbunny ci` fails before attempting downloads.

Locks older than v5 require a fresh `jetbunny install`. No implicit fallback to the
public npm registry occurs after an HTTP or authentication error.

## Parallel GETs

Metadata and tarball batches use `Transport::fetch_parallel`. The default implementation
runs up to eight single-hop GETs per batch by calling `Transport::fetch` concurrently.
An embedder with its own network scheduler can override `fetch_parallel` to handle the
whole batch. Both methods return one result per request in the original order.
Jetpack applies URL policy, credentials, and body limits to each redirect hop.

## Limitations

This provides a **client for npm-compatible registries**, not an embedded
registry server. Publishing (`npm publish`), Basic auth, automatic token
refresh, global `.npmrc`, mutual TLS, and npm-specific proxy settings are not
supported.
Only registry dependency sources are installed. Local directories, Git sources and
direct tarball-URL dependency specs remain unsupported.
