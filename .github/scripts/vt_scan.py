#!/usr/bin/env python3
"""Submit CI binaries to VirusTotal and summarise the result.

Why hash-first: the public API allows 4 requests/minute and 500/day. Looking a
file up by SHA-256 costs one request and needs no upload; a rebuilt-but-identical
binary is therefore free. Only an unknown hash is uploaded.

The API key is read from the environment and never printed. Nothing here can
fail a build unless --fail-on-detection is passed: a scanner false positive must
never block a release, it must be visible.

Usage:
    VT_API_KEY=... vt_scan.py [--fail-on-detection] [--rate 4] [--report PATH] FILE...
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

API = "https://www.virustotal.com/api/v3"
GUI = "https://www.virustotal.com/gui/file"
MAX_DIRECT_UPLOAD = 32 * 1024 * 1024  # VT direct-upload ceiling


class RateLimiter:
    """Enforce a minimum interval between API calls (public API: 4/min)."""

    def __init__(self, per_minute: int) -> None:
        self.interval = 60.0 / max(1, per_minute)
        self.last = 0.0

    def wait(self) -> None:
        delta = time.monotonic() - self.last
        if delta < self.interval:
            time.sleep(self.interval - delta)
        self.last = time.monotonic()


def log(msg: str) -> None:
    print(msg, flush=True)


def request(key: str, url: str, data: bytes | None = None, ctype: str | None = None,
            attempts: int = 4) -> tuple[int, dict]:
    """One API call with 429/5xx backoff. Returns (status, parsed-json)."""
    for attempt in range(1, attempts + 1):
        req = urllib.request.Request(url, data=data, method="POST" if data else "GET")
        req.add_header("x-apikey", key)
        req.add_header("Accept", "application/json")
        if ctype:
            req.add_header("Content-Type", ctype)
        try:
            with urllib.request.urlopen(req, timeout=120) as resp:
                return resp.status, json.load(resp)
        except urllib.error.HTTPError as exc:
            if exc.code == 404:
                return 404, {}
            body = exc.read().decode("utf-8", "replace")[:200]
            if exc.code in (429, 500, 502, 503, 504) and attempt < attempts:
                wait = 60 if exc.code == 429 else 10 * attempt
                log(f"  HTTP {exc.code}; retrying in {wait}s")
                time.sleep(wait)
                continue
            if exc.code == 401:
                log("  HTTP 401 — the VT_API_KEY secret is missing, wrong, or revoked")
            else:
                log(f"  HTTP {exc.code}: {body}")
            return exc.code, {}
        except (urllib.error.URLError, TimeoutError) as exc:
            if attempt < attempts:
                log(f"  network error ({exc}); retrying")
                time.sleep(10 * attempt)
                continue
            log(f"  network error: {exc}")
            return 0, {}
    return 0, {}


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def stats_of(attrs: dict) -> dict:
    return attrs.get("last_analysis_stats") or {}


def detections(attrs: dict) -> list[tuple[str, str]]:
    """(engine, verdict) for engines that called the file malicious/suspicious."""
    out = []
    for engine, res in (attrs.get("last_analysis_results") or {}).items():
        if res.get("category") in ("malicious", "suspicious"):
            out.append((engine, res.get("result") or res.get("category")))
    return sorted(out)


def upload(key: str, path: Path, limiter: RateLimiter) -> str | None:
    """Upload an unknown file. Returns the analysis id, or None on failure."""
    data = path.read_bytes()
    if len(data) > MAX_DIRECT_UPLOAD:
        log(f"  {path.name}: {len(data)} bytes exceeds the direct-upload limit; skipped")
        return None
    boundary = "----modman" + uuid.uuid4().hex
    head = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{path.name}"\r\n'
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode()
    tail = f"\r\n--{boundary}--\r\n".encode()
    limiter.wait()
    status, body = request(
        key, f"{API}/files", data=head + data + tail,
        ctype=f"multipart/form-data; boundary={boundary}",
    )
    if status == 200:
        return (((body.get("data") or {}).get("id")) or None)
    return None


def poll(key: str, analysis_id: str, limiter: RateLimiter, deadline_s: int = 480) -> dict:
    """Wait for an analysis to finish; returns its attributes (may lack stats)."""
    start = time.monotonic()
    delay = 20
    while time.monotonic() - start < deadline_s:
        time.sleep(delay)
        limiter.wait()
        status, body = request(key, f"{API}/analyses/{analysis_id}")
        attrs = (body.get("data") or {}).get("attributes") or {}
        state = attrs.get("status")
        if state == "completed":
            return attrs
        log(f"  analysis {state or 'queued'}")
        delay = min(delay + 10, 45)
    log("  analysis did not finish in time")
    return {}


def scan_one(key: str, path: Path, limiter: RateLimiter) -> dict:
    sha = sha256_of(path)
    row = {"file": path.name, "sha256": sha, "size": path.stat().st_size,
           "malicious": None, "total": None, "known": False, "uploaded": False,
           "engines": [], "link": f"{GUI}/{sha}"}

    limiter.wait()
    status, body = request(key, f"{API}/files/{sha}")
    attrs = (body.get("data") or {}).get("attributes") or {}
    if status == 200 and attrs:
        stats = stats_of(attrs)
        row.update(malicious=stats.get("malicious", 0), total=sum(
            stats.get(k, 0) for k in ("malicious", "suspicious", "undetected", "harmless")),
            known=True, engines=detections(attrs))
        log(f"  known to VT: {row['malicious']}/{row['total']} malicious")
        return row

    if status != 404:
        log(f"  lookup failed (HTTP {status}) — not uploading")
        return row

    log("  not seen by VT before; uploading")
    analysis_id = upload(key, path, limiter)
    if not analysis_id:
        return row
    row["uploaded"] = True
    attrs = poll(key, analysis_id, limiter)
    stats = attrs.get("stats") or {}
    if stats:
        row.update(malicious=stats.get("malicious", 0), total=sum(
            stats.get(k, 0) for k in ("malicious", "suspicious", "undetected", "harmless", "timeout")),
            known=True)
        # The analysis payload has per-engine results under `results`.
        row["engines"] = sorted(
            (eng, (res or {}).get("result") or "malicious")
            for eng, res in (attrs.get("results") or {}).items()
            if (res or {}).get("category") in ("malicious", "suspicious"))
        log(f"  scanned: {row['malicious']}/{row['total']} malicious")
    return row


def render(rows: list[dict], fail_on_detection: bool) -> tuple[str, int]:
    flagged = [r for r in rows if (r["malicious"] or 0) > 0]
    lines = ["## VirusTotal", ""]
    if not rows:
        lines += ["No files were scanned.", ""]
    else:
        lines += ["| file | size | detections | source |",
                  "|---|---|---|---|"]
        for r in rows:
            if r["known"] is None or r["malicious"] is None:
                verdict = "lookup failed"
            elif r["malicious"] == 0:
                verdict = f"**clean** 0/{r['total']}"
            else:
                verdict = f"**{r['malicious']}/{r['total']}**"
            src = "uploaded now" if r.get("uploaded") else "already on VT"
            lines.append(f"| [`{r['file']}`]({r['link']}) | {r['size'] / 1e6:.2f} MB | {verdict} | {src} |")
        lines.append("")
        for r in rows:
            if r["malicious"]:
                detail = ", ".join(f"{eng}: {res}" for eng, res in r["engines"][:12])
                lines.append(f"- `{r['file']}` — {detail}")
        if not flagged:
            lines.append("No engine flagged any file.")
        lines.append("")
        lines.append("Permalinks above stay valid as engines re-scan; a clean result is not a "
                     "safety guarantee, and a single detection is usually a false positive on an "
                     "unsigned binary.")
        lines.append("")

    text = "\n".join(lines)
    return text, (1 if (fail_on_detection and flagged) else 0)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--rate", type=int, default=4, help="API calls per minute (public API: 4)")
    ap.add_argument("--report", default="virustotal-report.md")
    ap.add_argument("--fail-on-detection", action="store_true",
                    help="Exit non-zero if any engine flags a file (off by default)")
    args = ap.parse_args()

    key = os.environ.get("VT_API_KEY", "").strip()
    if not key:
        log("VT_API_KEY is not set — skipping the scan")
        text = "## VirusTotal\n\nSkipped: the `VT_API_KEY` secret is not configured.\n"
        Path(args.report).write_text(text)
        summary = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary:
            with open(summary, "a") as fh:
                fh.write(text)
        return 0

    limiter = RateLimiter(args.rate)
    rows = []
    for raw in args.files:
        path = Path(raw)
        if not path.is_file():
            log(f"skip (not a file): {raw}")
            continue
        log(f"{path.name} ({path.stat().st_size:,} bytes)")
        try:
            rows.append(scan_one(key, path, limiter))
        except Exception as exc:  # never let reporting kill the job
            log(f"  scan failed: {exc!r}")
            rows.append({"file": path.name, "sha256": "", "size": path.stat().st_size,
                         "malicious": None, "total": None, "known": None,
                         "engines": [], "link": ""})

    text, code = render(rows, args.fail_on_detection)
    Path(args.report).write_text(text + "\n")
    log("")
    log(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as fh:
            fh.write(text + "\n")
    return code


if __name__ == "__main__":
    sys.exit(main())
