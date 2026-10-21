# 0021 Egress proxy, Mailpit, and what they cannot see

Status: accepted · Date: 2026-10-21

## Context

§4.1 cannot block egress without a sandbox, and B13 in M6 needs the egress channel as a hard gate. Issue 8 moves a minimal proxy and mailbox from M8 to M3.

## Decision

- A forward proxy on `hudsucker` with a per-run CA from `rcgen`. The CA certificate is written to `runs/<run_id>/egress/ca.pem`; **its private key is never written anywhere** and lives only in the proxy's issuer. A run directory outlives the run, so a key on disk would let whoever holds it impersonate every host in the next run.
- Deny by default: a host not in `[egress] allow` gets `502` with `denied: true`, recorded with its request body. A denial is a *response*, not a dropped connection, because the check needs the attempt in the record. `allow` matches a host exactly or as a subdomain suffix (`api.example.com` allows `cdn.api.example.com`, never `evil-api.example.com`).
- Every request is logged with host, method, path, status, byte counts, request and response bodies as blobs, and a monotonic counter instead of a clock (ADR-0009). `NO_PROXY` carries loopback and the database host, so those connections bypass the proxy and are invisible to this channel.
- The app gets `HTTP_PROXY`, `HTTPS_PROXY` and their lowercase forms, `NO_PROXY`, `NODE_EXTRA_CA_CERTS`, `NODE_USE_ENV_PROXY=1`, `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` and `CURL_CA_BUNDLE`. The CA variables point at the *run's* CA, not the system roots: only proxied TLS validates, which is the intent, since an unproxied TLS connection is one substance did not see.
- Mailpit catches the app's mail on free SMTP and HTTP ports; `[mail] env` is what tells the app where they are. A missing binary makes the email channel `not_measured("tool missing: mailpit")`, never an empty list.

## Consequences

- The manifest records `egress_enforced: false`. Clients that ignore proxy variables are **invisible**, not blocked: Node's `undici`/native `fetch` before `NODE_USE_ENV_PROXY` (older Node), any SDK that builds its own `reqwest`/`hyper`/`http.Client` with a fixed connector, Go programs that replace `http.DefaultTransport`, statically linked binaries with their own TLS stack, and anything speaking a non-HTTP protocol (raw TCP, DNS, SMTP to a real host). A run that needs certainty about one of those must assert it another way.
- A denial costs the app a failed request it can see, so an app that treats any network error as fatal may fail earlier than it would with the network open. That is the intended trade: the run is meant to be closed.
- MITM means pinned certificates inside the app will fail against the proxy, and that failure is indistinguishable from a real interception. Apps that pin are out of scope until M8 adds stubs.
