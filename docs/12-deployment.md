# Deployment (home server)

Target: a Linux box on your LAN, x86-64 or aarch64, 2–8 cores, 4–32 GB RAM, no public
ingress required.

## Artifact

One binary with the UI embedded:

```bash
cd apps/web && pnpm install && pnpm build          # → apps/web/dist
cargo build --release --features embed-web          # → target/release/jobseeker
```

Optional runtime dependencies, each degrading gracefully when absent:

| Dependency | Needed for | Absent → |
|---|---|---|
| `typst` binary | resume/cover-letter PDFs | source-only output (`resume.renderer = "none"`) |
| Ollama (or an API key) | LLM extraction, phrasing, narratives | deterministic extraction only, `extraction_partial = true` |
| an embedding model | semantic similarity | matching falls back to taxonomy/lexical, flagged |

## Filesystem

```
/opt/jobseeker/jobseeker            binary
/etc/jobseeker/config.toml          config      (0640 jobseeker:jobseeker)
/etc/jobseeker/secrets.env          API keys    (0600)
/srv/jobseeker/data/                data dir    (0750) — jobs/, documents/, jobseeker.db
```

Put `$DATA_DIR` on an SSD. SQLite WAL on spinning rust or a network filesystem is a bad
time; NFS in particular breaks SQLite's locking assumptions and is unsupported.

## systemd

```ini
# /etc/systemd/system/jobseeker.service
[Unit]
Description=Jobseeker
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
User=jobseeker
Group=jobseeker
ExecStart=/opt/jobseeker/jobseeker serve --config /etc/jobseeker/config.toml
EnvironmentFile=/etc/jobseeker/secrets.env
Restart=on-failure
RestartSec=5s
WatchdogSec=60s

# hardening
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
ReadWritePaths=/srv/jobseeker/data
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=true
LockPersonality=true
MemoryDenyWriteExecute=true
SystemCallFilter=@system-service
SystemCallArchitectures=native
CapabilityBoundingSet=
LimitNOFILE=8192

[Install]
WantedBy=multi-user.target
```

`Type=notify` + `WatchdogSec` means a wedged process gets restarted rather than silently
accepting connections it cannot serve.

## Network exposure

Default bind is `127.0.0.1:8787` (NFR-S-06). Pick one, in decreasing order of preference:

1. **Tailscale / WireGuard** — bind to the tailnet address, `auth.mode = "token"`, reachable
   from your phone and laptop anywhere, nothing on the public internet. Recommended.
2. **LAN + reverse proxy with TLS** — Caddy in front, `auth.mode = "password"`. Needed
   anyway if you want the browser extension to talk to it over HTTPS from a different device.
3. **Public internet** — not recommended. If you must: TLS, `auth.mode = "password"`, a
   fail2ban-equivalent on `/auth/login`, and accept that a job-search database is a
   privacy-sensitive target.

```caddyfile
jobs.home.arpa {
  encode zstd gzip
  reverse_proxy 127.0.0.1:8787 {
    flush_interval -1                 # required: SSE must not be buffered
  }
}
```

`flush_interval -1` matters — a buffering proxy silently breaks live progress updates.

## Extension pairing

```bash
jobseeker pair --name "Firefox on laptop"
# → paste this into the extension options page:
#   server: http://jobs.home.arpa:8787
#   token:  jst_9f2c...        (shown once; stored hashed)
```

The extension's origin is added to the CORS allowlist automatically. Tokens are per-device,
scoped to `ingest`, and revocable from Settings.

## Container (alternative)

```dockerfile
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --features embed-web

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/jobseeker /usr/local/bin/jobseeker
VOLUME /data
ENV JOBSEEKER__DATA__DIR=/data
EXPOSE 8787
ENTRYPOINT ["jobseeker","serve"]
```

Web assets are built in a separate `node` stage in the real Dockerfile. Bind-mount the data
volume from the host — do not keep SQLite inside the container's overlay filesystem.

## Backups

`jobseeker backup` writes `backups/<timestamp>/`:

- `jobseeker.db` via `VACUUM INTO` — a consistent snapshot without stopping the service.
- `files.tar.zst` of `jobs/`, `profiles/`, `documents/`, `applications/`, `taxonomy/`.
- `manifest.json` with schema version, counts, and hashes.

```
# /etc/systemd/system/jobseeker-backup.timer → daily 03:00
ExecStart=/opt/jobseeker/jobseeker backup --keep 14 --config /etc/jobseeker/config.toml
```

**Restore drill** (run it once, before you need it):

```bash
systemctl stop jobseeker
mv /srv/jobseeker/data /srv/jobseeker/data.bak
mkdir -p /srv/jobseeker/data && tar -C /srv/jobseeker/data -xf backups/<ts>/files.tar.zst
jobseeker reconcile --from-files          # rebuild the DB from files alone
jobseeker reconcile --check               # must report zero drift
systemctl start jobseeker
```

Rebuilding from files rather than copying the `.db` is deliberate: it verifies that the file
tree is genuinely sufficient, which is the storage design's central claim.

## Upgrades

```bash
jobseeker backup
systemctl stop jobseeker
install -m755 jobseeker /opt/jobseeker/jobseeker
jobseeker migrate --config /etc/jobseeker/config.toml     # or let serve do it
systemctl start jobseeker && curl -fsS localhost:8787/readyz
```

Migrations are forward-only. The binary refuses to start against a newer schema than it
knows (NFR-O-06), so an accidental downgrade fails loudly instead of corrupting data.

## Monitoring

- `/healthz` liveness, `/readyz` (DB reachable, migrations current, queue not wedged).
- `/metrics` for Prometheus: `jobseeker_ingest_total{source,outcome}`,
  `jobseeker_task_duration_seconds{kind}`, `jobseeker_queue_depth{status}`,
  `jobseeker_llm_tokens_total{model,purpose}`, `jobseeker_llm_cost_micros_total`,
  `jobseeker_http_request_duration_seconds{route,status}`,
  `jobseeker_extraction_confidence` (histogram).
- Alerts worth having: `readyz` failing, `queue_depth{status="failed"} > 0`,
  tasks running longer than the lease, extraction confidence trending down (a site changed
  its markup and an adapter is silently degrading).
- Logs are structured; `journalctl -u jobseeker -f | jq` when `log.format = "json"`.

## Sizing

| Workload | CPU | RAM | Disk |
|---|---|---|---|
| Deterministic extraction only | 2 cores | 512 MB | ~2 GB / 5k jobs |
| + local 7B LLM on CPU | 8 cores | 8 GB | + 5 GB model |
| + local 14B LLM on GPU | 4 cores + 12 GB VRAM | 8 GB | + 9 GB model |
| + cloud LLM | 2 cores | 512 MB | ~2 GB |

Idle should sit near 0% CPU and under 100 MB RSS (NFR-P-05). If it does not, the worker poll
loop is misconfigured.
