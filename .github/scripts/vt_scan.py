#!/usr/bin/env python3
"""Submit CI binaries to VirusTotal and summarise the result.

Why hash-first: the public API allows 4 requests/minute and 500/day. Looking a
file up by SHA-256 costs one request and needs no upload; a rebuilt-but-identical
binary is therefore free. Only an unknown hash is uploaded.

Why concurrent: a submitted file takes 50-80 seconds to be analysed, and the
service keeps analysing four files at once. Scanning one file at a time leaves
three of those slots idle for the whole wait, so files are scanned four at a
time, each worker owning one file from lookup to verdict. The 4/minute limit is
on REQUESTS, not analyses, so the workers share one rolling-window limiter and
the quota is never exceeded: four uploads in one minute start four analyses, and
the verdict lookups go out in the next window while those analyses are still
running.

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
import threading
import time
import urllib.error
import urllib.request
import uuid
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

API = os.environ.get("VT_API_BASE", "https://www.virustotal.com/api/v3")
GUI = "https://www.virustotal.com/gui/file"
MAX_DIRECT_UPLOAD = 32 * 1024 * 1024  # VT direct-upload ceiling
# The quota is per minute; the window is only a knob so tests can run in seconds.
WINDOW = float(os.environ.get("VT_WINDOW_SECONDS", "60"))
# How long to let a fresh upload settle before asking for its verdict once.
SETTLE = int(os.environ.get("VT_SETTLE_SECONDS", "45"))


class RateLimiter:
    """At most `per_minute` calls in any rolling window, across all threads.

    The public API allows 4 requests/minute total, so concurrency does not raise
    the ceiling; it removes the serial latency and lets a burst of four go at
    once instead of one call per 15 seconds. Because a burst can still trip a
    429 if the server counts windows differently, a 429 puts every thread on a
    shared cooldown: four independent retry loops would otherwise burn the next
    window four times over.
    """

    def __init__(self, per_minute: int, window: float = WINDOW) -> None:
        self.capacity = max(1, per_minute)
        self.window = max(0.1, window)
        self.times: deque[float] = deque()
        self.lock = threading.Lock()
        self.cooldown_until = 0.0

    def wait(self) -> None:
        while True:
            with self.lock:
                now = time.monotonic()
                while self.times and now - self.times[0] >= self.window:
                    self.times.popleft()
                if now < self.cooldown_until:
                    pause = self.cooldown_until - now
                elif len(self.times) < self.capacity:
                    self.times.append(now)
                    return
                else:
                    pause = self.window - (now - self.times[0])
                # Re-check in short steps so a freed or cooled-down window is
                # noticed promptly by every waiter.
                pause = min(max(pause, 0.05), 5.0)
            time.sleep(pause)

    def penalise(self, seconds: float | None = None) -> None:
        """A 429 means the window is spent: hold every thread off, once."""
        with self.lock:
            span = self.window / 2 if seconds is None else seconds
            self.cooldown_until = max(self.cooldown_until, time.monotonic() + span)


def log(msg: str) -> None:
    print(msg, flush=True)


def request(key: str, url: str, limiter: RateLimiter, data: bytes | None = None,
            ctype: str | None = None, attempts: int = 4,
            rate_deadline_s: float = 600.0) -> tuple[int, dict]:
    """One API call. Retries 5xx/network failures a few times; 429s do NOT count.

    A rate-limit retry is not a failure: the quota simply has not refilled. It
    gets its own deadline instead of the shared attempt budget, because burning
    four attempts on 429s used to drop a file from the report entirely on a
    busy window.
    """
    attempt = 0
    deadline = time.monotonic() + rate_deadline_s
    while True:
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
            if exc.code == 429:
                if time.monotonic() > deadline:
                    log("  429 persisted past the deadline; giving up on this call")
                    return 429, {}
                # Shared: nobody spends the next window while the quota is spent.
                limiter.penalise()
                log("  HTTP 429; quota window is spent, holding all workers")
                continue
            if exc.code in (500, 502, 503, 504) and attempt < attempts:
                attempt += 1
                wait = 10 * attempt
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
                attempt += 1
                log(f"  network error ({exc}); retrying")
                time.sleep(10 * attempt)
                continue
            log(f"  network error: {exc}")
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
        key, f"{API}/files", limiter, data=head + data + tail,
        ctype=f"multipart/form-data; boundary={boundary}",
    )
    if status == 200:
        return (((body.get("data") or {}).get("id")) or None)
    return None


def await_result(key: str, sha: str, limiter: RateLimiter, say=None,
                 settle: int | None = None, rounds: int = 2) -> dict:
    """Wait, then look the hash up. Cheaper than polling the analysis.

    A poll costs one request per round per file; a hash lookup costs one request
    and returns the same `last_analysis_stats` and `last_analysis_results` once
    the scan is done. With the quota at 4 requests/minute, the request count is
    the runtime, so two settling lookups beat four or five polls every time.
    """
    settle = SETTLE if settle is None else settle
    for attempt in range(1, rounds + 1):
        time.sleep(settle)
        limiter.wait()
        status, body = request(key, f"{API}/files/{sha}", limiter)
        attrs = (body.get("data") or {}).get("attributes") or {}
        stats = stats_of(attrs)
        if status == 200 and sum(stats.get(k, 0) for k in ("malicious", "suspicious",
                                                           "undetected", "harmless",
                                                           "timeout")) > 0:
            return attrs
        if say:
            say(f"scan not reported yet (round {attempt}/{rounds})")
    return {}


def scan_one(key: str, path: Path, limiter: RateLimiter) -> dict:
    def say(msg: str) -> None:
        log(f"  {path.name}: {msg}")

    sha = sha256_of(path)
    row = {"file": path.name, "sha256": sha, "size": path.stat().st_size,
           "malicious": None, "total": None, "known": False, "uploaded": False,
           "engines": [], "link": f"{GUI}/{sha}"}

    limiter.wait()
    status, body = request(key, f"{API}/files/{sha}", limiter)
    attrs = (body.get("data") or {}).get("attributes") or {}
    if status == 200 and attrs:
        stats = stats_of(attrs)
        row.update(malicious=stats.get("malicious", 0), total=sum(
            stats.get(k, 0) for k in ("malicious", "suspicious", "undetected", "harmless")),
            known=True, engines=detections(attrs))
        say(f"known to VT: {row['malicious']}/{row['total']} malicious")
        return row

    if status != 404:
        say(f"lookup failed (HTTP {status}) — not uploading")
        return row

    say("not seen by VT before; uploading")
    analysis_id = upload(key, path, limiter)
    if not analysis_id:
        return row
    row["uploaded"] = True
    attrs = await_result(key, sha, limiter, say)
    stats = stats_of(attrs)
    if stats:
        row.update(malicious=stats.get("malicious", 0), total=sum(
            stats.get(k, 0) for k in ("malicious", "suspicious", "undetected",
                                      "harmless", "timeout")),
            known=True, engines=detections(attrs))
        say(f"scanned: {row['malicious']}/{row['total']} malicious")
    else:
        say("scan still queued at VT; it will be on the page on the next run")
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
    ap.add_argument("--workers", type=int, default=0,
                    help="Parallel scans (default: same as --rate)")
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
    paths = []
    for raw in args.files:
        p = Path(raw)
        if not p.is_file():
            log(f"skip (not a file): {raw}")
            continue
        paths.append(p)

    # One worker per allowed request: the quota is per minute, so more workers
    # than that would only queue behind the limiter while burning memory on
    # uploads. Fewer files than the quota means fewer workers.
    workers = max(1, min(args.workers or args.rate, len(paths) or 1))
    started = time.monotonic()
    log(f"scanning {len(paths)} file(s) with {workers} worker(s), "
        f"{args.rate} request(s) per {WINDOW:.0f}s")
    for p in paths:
        log(f"{p.name} ({p.stat().st_size:,} bytes)")

    def work(path: Path) -> dict:
        try:
            return scan_one(key, path, limiter)
        except Exception as exc:  # never let reporting kill the job
            log(f"  {path.name}: scan failed: {exc!r}")
            return {"file": path.name, "sha256": "", "size": path.stat().st_size,
                    "malicious": None, "total": None, "known": None,
                    "engines": [], "link": ""}

    rows: list[dict] = []
    with ThreadPoolExecutor(max_workers=workers) as pool:
        # Results are collected by index so the report keeps the caller's order
        # regardless of which file finished first.
        futures = {pool.submit(work, p): i for i, p in enumerate(paths)}
        ordered: dict[int, dict] = {}
        for fut, idx in futures.items():
            ordered[idx] = fut.result()
        rows = [ordered[i] for i in sorted(ordered)]
    log(f"scan finished in {time.monotonic() - started:.0f}s")

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
