// Copyright (c) 2026 Coldmind AB <https://coldmind.com>
// SPDX-License-Identifier: MIT

//! The network boundary. jetpack owns no HTTP stack: the embedder supplies a [`Transport`]
//! that performs single GET hops, and [`fetch_all`] applies jetpack's policy on top of it:
//! redirects, per-hop credentials, the URL policy, status checks and body limits.
use anyhow::{anyhow, bail, Context, Result};
use std::{sync::Mutex, thread};
use url::Url;

use crate::config::RegistryConfig;

/// Redirect hops followed per request before failing.
pub const MAX_REDIRECTS: usize = 5;

/// Maximum number of simultaneous GETs in a batch using the default scheduler.
pub const MAX_PARALLEL_GETS: usize = 8;

/// One GET request. The transport sends exactly these headers and follows no redirect.
#[derive(Clone)]
pub struct Request {
    pub url: String,
    /// `Accept` header value, if any.
    pub accept: Option<&'static str>,
    /// `Authorization` header value, if any. Never logged.
    pub authorization: Option<String>,
    /// Maximum body bytes. A longer body must fail the request rather than be truncated.
    pub limit: u64,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Request")
            .field("url", &redact(&self.url))
            .field("accept", &self.accept)
            .field(
                "authorization",
                &self.authorization.as_ref().map(|_| "<redacted>"),
            )
            .field("limit", &self.limit)
            .finish()
    }
}

/// The typed outcome when a transport stops reading an oversized body.
#[derive(Debug)]
pub struct BodyTooLarge {
    pub limit: u64,
}

impl std::fmt::Display for BodyTooLarge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "response body exceeds {} bytes", self.limit)
    }
}

impl std::error::Error for BodyTooLarge {}

/// The response to one hop.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    /// The `Location` header of a redirect.
    pub location: Option<String>,
    pub body: Vec<u8>,
}

/// Performs single-hop GETs. Both methods return one result per request, in request
/// order. Oversized bodies should return a [`BodyTooLarge`] error.
pub trait Transport: Send + Sync {
    /// Executes a batch of requests. The default scheduler calls this method
    /// concurrently with one request per call.
    fn fetch(&self, batch: Vec<Request>) -> Vec<Result<Response>>;

    /// Execute independent GETs concurrently. The default allows up to
    /// [`MAX_PARALLEL_GETS`] per batch.
    /// Transports with their own batching or event loop can override this method and
    /// delegate to [`Transport::fetch`] for the entire batch.
    fn fetch_parallel(&self, batch: Vec<Request>) -> Vec<Result<Response>> {
        let count = batch.len();
        match count {
            0 => return Vec::new(),
            1 => return self.fetch(batch),
            _ => {}
        }
        let mut results = Vec::with_capacity(count);
        results.resize_with(count, || None);
        let jobs = Mutex::new(batch.into_iter().zip(&mut results));
        let run = || loop {
            let job = jobs.lock().expect("GET queue lock").next();
            let Some((request, result)) = job else {
                break;
            };
            let mut responses = self.fetch(vec![request]);
            *result = Some(if responses.len() == 1 {
                responses.pop().expect("one response")
            } else {
                Err(anyhow!("transport returned a wrong number of responses"))
            });
        };
        thread::scope(|scope| {
            for _ in 1..count.min(MAX_PARALLEL_GETS) {
                if thread::Builder::new().spawn_scoped(scope, run).is_err() {
                    break;
                }
            }
            run();
        });
        results
            .into_iter()
            .map(|result| result.expect("GET worker processed every request"))
            .collect()
    }
}

/// What a caller asks for: a URL, its `Accept` header and a body limit.
#[derive(Clone, Debug)]
pub struct Fetch {
    pub url: String,
    pub accept: Option<&'static str>,
    pub limit: u64,
}

/// Fetches entries concurrently, following up to [`MAX_REDIRECTS`] redirects
/// (301, 302, 303, 307, 308) per entry. Every hop must pass
/// [`crate::registry::validate_url`], and its bearer token is
/// chosen for that hop's URL, so a redirect to another origin never carries the original
/// registry's token. A final status other than 2xx is an error. Results are in input order.
pub fn fetch_all(
    transport: &dyn Transport,
    config: Option<&RegistryConfig>,
    fetches: &[Fetch],
) -> Vec<Result<Vec<u8>>> {
    let mut results: Vec<Option<Result<Vec<u8>>>> = (0..fetches.len()).map(|_| None).collect();
    // (index, current URL, hops taken)
    let mut pending: Vec<(usize, String, usize)> = fetches
        .iter()
        .enumerate()
        .map(|(index, fetch)| (index, fetch.url.clone(), 0))
        .collect();
    while !pending.is_empty() {
        let mut sent = Vec::new();
        let mut batch = Vec::new();
        for (index, url, hops) in pending.drain(..) {
            match request(config, &fetches[index], &url) {
                Ok(request) => {
                    sent.push((index, url, hops));
                    batch.push(request);
                }
                Err(error) => results[index] = Some(Err(error)),
            }
        }
        if batch.is_empty() {
            continue;
        }
        let responses = transport.fetch_parallel(batch);
        if responses.len() != sent.len() {
            for (index, _, _) in sent {
                results[index] = Some(Err(anyhow!(
                    "transport returned a wrong number of responses"
                )));
            }
            break;
        }
        for ((index, url, hops), response) in sent.into_iter().zip(responses) {
            let outcome = response
                .and_then(|response| {
                    let limit = fetches[index].limit;
                    if response.body.len() as u64 > limit {
                        return Err(BodyTooLarge { limit }.into());
                    }
                    Ok(response)
                })
                .with_context(|| format!("GET {}", redact(&url)));
            match outcome {
                Ok(response) if (200..300).contains(&response.status) => {
                    results[index] = Some(Ok(response.body));
                }
                Ok(response) if matches!(response.status, 301 | 302 | 303 | 307 | 308) => {
                    let next = response
                        .location
                        .as_deref()
                        .ok_or_else(|| {
                            anyhow!(
                                "redirect {} without Location from {}",
                                response.status,
                                redact(&url)
                            )
                        })
                        .and_then(|location| {
                            Url::parse(&url)?.join(location).with_context(|| {
                                format!("invalid redirect Location from {}", redact(&url))
                            })
                        });
                    match next {
                        Ok(_) if hops >= MAX_REDIRECTS => {
                            results[index] = Some(Err(anyhow!(
                                "more than {MAX_REDIRECTS} redirects from {}",
                                redact(&fetches[index].url)
                            )));
                        }
                        Ok(next) => pending.push((index, next.to_string(), hops + 1)),
                        Err(error) => results[index] = Some(Err(error)),
                    }
                }
                Ok(response) => {
                    results[index] = Some(Err(anyhow!(
                        "HTTP {} from {}",
                        response.status,
                        redact(&url)
                    )));
                }
                Err(error) => results[index] = Some(Err(error)),
            }
        }
    }
    results
        .into_iter()
        .map(|result| result.unwrap_or_else(|| Err(anyhow!("request was not completed"))))
        .collect()
}

fn request(config: Option<&RegistryConfig>, fetch: &Fetch, url: &str) -> Result<Request> {
    crate::registry::validate_url(url)?;
    let parsed = Url::parse(url)?;
    if let Some(config) = config {
        config.check_destination(&parsed)?;
    }
    let authorization = match config.and_then(|config| config.token_for(&parsed)) {
        Some(token) if token.bytes().any(|b| b < 0x20 || b == 0x7f) => {
            bail!("invalid registry bearer token")
        }
        Some(token) => Some(format!("Bearer {token}")),
        None => None,
    };
    Ok(Request {
        url: url.to_owned(),
        accept: fetch.accept,
        authorization,
        limit: fetch.limit,
    })
}

/// The URL without its query, for error messages.
fn redact(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => "<invalid URL>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    /// Serves canned hops and records the Authorization header of each.
    struct Canned {
        hops: Mutex<Vec<(String, Option<String>)>>,
    }

    impl Transport for Canned {
        fn fetch(&self, batch: Vec<Request>) -> Vec<Result<Response>> {
            batch
                .into_iter()
                .map(|request| {
                    self.hops
                        .lock()
                        .unwrap()
                        .push((request.url.clone(), request.authorization.clone()));
                    Ok(match request.url.as_str() {
                        "https://registry.test/a.tgz" => Response {
                            status: 302,
                            location: Some("https://cdn.test/a.tgz".into()),
                            body: Vec::new(),
                        },
                        "https://cdn.test/a.tgz" => Response {
                            status: 200,
                            location: None,
                            body: b"tarball".to_vec(),
                        },
                        "https://registry.test/loop" => Response {
                            status: 301,
                            location: Some("/loop".into()),
                            body: Vec::new(),
                        },
                        _ => Response {
                            status: 404,
                            location: None,
                            body: Vec::new(),
                        },
                    })
                })
                .collect()
        }
    }

    fn fetch(url: &str) -> Fetch {
        Fetch {
            url: url.into(),
            accept: None,
            limit: 1024,
        }
    }

    #[test]
    fn parallel_gets_are_bounded_and_return_results_in_input_order() {
        const TIMEOUT: Duration = Duration::from_secs(10);
        type Started = (usize, mpsc::Sender<()>);
        struct Gated(mpsc::Sender<Started>);

        impl Transport for Gated {
            fn fetch(&self, batch: Vec<Request>) -> Vec<Result<Response>> {
                let [request]: [Request; 1] = batch.try_into().unwrap();
                let index = request.url.rsplit('/').next().unwrap().parse().unwrap();
                let (release, wait) = mpsc::channel();
                self.0.send((index, release)).unwrap();
                vec![wait
                    .recv_timeout(TIMEOUT)
                    .context("GET was not released")
                    .map(|()| Response {
                        status: if index == 1 { 404 } else { 200 },
                        location: None,
                        body: index.to_string().into_bytes(),
                    })]
            }
        }

        let (start, started) = mpsc::channel();
        let transport = Gated(start);
        let count = MAX_PARALLEL_GETS + 1;
        let fetches: Vec<_> = (0..count)
            .map(|index| fetch(&format!("https://registry.test/{index}")))
            .collect();
        thread::scope(|scope| {
            let fetching = scope.spawn(|| fetch_all(&transport, None, &fetches));
            let mut in_flight: Vec<_> = (0..MAX_PARALLEL_GETS)
                .map(|_| started.recv_timeout(TIMEOUT).unwrap())
                .collect();
            assert!(matches!(started.try_recv(), Err(mpsc::TryRecvError::Empty)));
            in_flight.sort_by_key(|(index, _)| *index);
            // Keep the first request blocked while a later one frees a worker.
            in_flight.pop().unwrap().1.send(()).unwrap();
            let (index, release) = started.recv_timeout(TIMEOUT).unwrap();
            assert_eq!(index, MAX_PARALLEL_GETS);
            release.send(()).unwrap();
            for (_, release) in in_flight {
                release.send(()).unwrap();
            }
            let results = fetching.join().unwrap();
            assert_eq!(results.len(), count);
            for (index, result) in results.into_iter().enumerate() {
                if index == 1 {
                    assert!(result.unwrap_err().to_string().contains("HTTP 404"));
                } else {
                    assert_eq!(result.unwrap(), index.to_string().into_bytes());
                }
            }
        });
    }

    #[test]
    fn redirect_to_another_origin_drops_the_registry_token() {
        let config = RegistryConfig::single("https://registry.test/")
            .unwrap()
            .with_token("//registry.test/:_authToken", "secret")
            .unwrap();
        let transport = Canned {
            hops: Mutex::new(Vec::new()),
        };
        let results = fetch_all(
            &transport,
            Some(&config),
            &[fetch("https://registry.test/a.tgz")],
        );
        assert_eq!(results[0].as_ref().unwrap(), b"tarball");
        let hops = transport.hops.lock().unwrap();
        assert_eq!(hops[0].1.as_deref(), Some("Bearer secret"));
        assert_eq!(hops[1], ("https://cdn.test/a.tgz".to_owned(), None));
    }

    #[test]
    fn redirect_loops_and_error_statuses_fail() {
        let transport = Canned {
            hops: Mutex::new(Vec::new()),
        };
        let results = fetch_all(
            &transport,
            None,
            &[
                fetch("https://registry.test/loop"),
                fetch("https://registry.test/missing"),
            ],
        );
        assert!(results[0]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("redirects"));
        assert!(results[1]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("HTTP 404"));
        assert_eq!(transport.hops.lock().unwrap().len(), 1 + MAX_REDIRECTS + 1);
    }

    #[test]
    fn request_debug_redacts_credentials_and_query_values() {
        let request = Request {
            url: "https://example.test/package?key=fixture-query-secret".into(),
            authorization: Some("Bearer fixture-bearer-secret".into()),
            accept: None,
            limit: 1024,
        };
        let output = format!("{request:?}");
        assert!(output.contains("https://example.test/package"));
        assert!(!output.contains("fixture-query-secret"));
        assert!(!output.contains("fixture-bearer-secret"));
    }

    #[test]
    fn rejects_bodies_larger_than_the_requested_limit() {
        let transport = Canned {
            hops: Mutex::new(Vec::new()),
        };
        let fetch = Fetch {
            limit: 5,
            ..fetch("https://cdn.test/a.tgz")
        };
        let result = fetch_all(&transport, None, &[fetch]).pop().unwrap();
        assert!(format!("{:#}", result.unwrap_err()).contains("response body exceeds 5 bytes"));
    }
}
