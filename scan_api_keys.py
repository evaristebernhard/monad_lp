#!/usr/bin/env python3
"""Find GitHub files containing likely plaintext API-key assignments.

Only matching file URLs are written to stdout (or --output). Secret values,
file contents, and matching lines are never printed or stored.
"""

from __future__ import annotations

import argparse
import re
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Iterable
from urllib.error import HTTPError, URLError
from urllib.parse import quote, unquote, urlparse
from urllib.request import Request, urlopen


DEFAULT_INPUT = "geckoterminal_urls_200.txt"
DEFAULT_TIMEOUT = 20
MAX_BYTES = 5 * 1024 * 1024

# Covers common spellings such as api_key, api-key, apiKey, and APIKEY.
API_KEY_ASSIGNMENT = re.compile(
    r"""
    \bapi[_-]?key\b
    \s*(?:=|:|=>)\s*
    (?:
        (?P<quote>[\"']) (?P<quoted>[^\"'\r\n]{8,}) (?P=quote)
        |
        (?P<bare>[A-Za-z0-9_./+=:-]{16,})
    )
    """,
    re.IGNORECASE | re.VERBOSE,
)

PLACEHOLDER_VALUES = {
    "api_key",
    "apikey",
    "your_api_key",
    "your-api-key",
    "yourapikey",
    "replace_me",
    "replace-me",
    "changeme",
    "change_me",
    "example",
    "test",
    "testing",
    "dummy",
    "placeholder",
    "none",
    "null",
}


def github_blob_to_raw(url: str) -> str:
    """Convert a github.com/.../blob/... URL to its raw-content URL."""
    parsed = urlparse(url.strip())
    if parsed.netloc.lower() == "raw.githubusercontent.com":
        return url.strip()

    parts = [unquote(part) for part in parsed.path.split("/") if part]
    if len(parts) < 5 or parts[2] != "blob":
        raise ValueError("unsupported GitHub file URL")

    owner, repo, _, revision = parts[:4]
    file_path = "/".join(quote(part, safe="@!$&'()*+,;=:-._~") for part in parts[4:])
    return f"https://raw.githubusercontent.com/{owner}/{repo}/{quote(revision, safe='')}/{file_path}"


def looks_like_plaintext(value: str) -> bool:
    """Reject obvious placeholders and environment-variable references."""
    normalized = value.strip().strip("`").lower()
    if normalized in PLACEHOLDER_VALUES:
        return False
    if any(marker in normalized for marker in (
        "${",
        "process.env",
        "os.getenv",
        "getenv(",
        "env[")):
        return False
    if normalized.startswith(("<", "your-", "your_", "replace", "change")):
        return False
    if set(normalized) <= {"x", "-", "_", "."}:
        return False
    return True


def contains_plaintext_api_key(text: str) -> bool:
    """Return whether text contains a likely literal API-key assignment."""
    return any(
        looks_like_plaintext(match.group("quoted") or match.group("bare") or "")
        for match in API_KEY_ASSIGNMENT.finditer(text)
    )


def fetch_and_scan(url: str, timeout: int) -> tuple[str, bool, str | None]:
    """Fetch and scan one URL without returning its contents."""
    try:
        raw_url = github_blob_to_raw(url)
        request = Request(
            raw_url,
            headers={"User-Agent": "api-key-exposure-audit/1.0", "Accept": "text/plain"},
        )
        with urlopen(request, timeout=timeout) as response:
            content_type = response.headers.get_content_type()
            if content_type not in {"text/plain", "text/html", "application/json", "application/javascript"}:
                return url, False, None
            data = response.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                return url, False, "file exceeds scan size limit"
            charset = response.headers.get_content_charset() or "utf-8"
            text = data.decode(charset, errors="replace")
        return url, contains_plaintext_api_key(text), None
    except (HTTPError, URLError, TimeoutError, UnicodeError, ValueError) as exc:
        # The exception text is deliberately not printed; it can contain response details.
        return url, False, type(exc).__name__


def read_urls(path: Path) -> Iterable[str]:
    seen: set[str] = set()
    for line in path.read_text(encoding="utf-8").splitlines():
        url = line.strip()
        if url and not url.startswith("#") and url not in seen:
            seen.add(url)
            yield url


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("-i", "--input", default=DEFAULT_INPUT, help="URL list, one URL per line")
    parser.add_argument("-o", "--output", help="write matching URLs to this file")
    parser.add_argument("-j", "--workers", type=int, default=8, help="parallel downloads (default: 8)")
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT, help="request timeout in seconds")
    parser.add_argument("--show-errors", action="store_true", help="show failed URL and error type on stderr")
    args = parser.parse_args()

    if args.workers < 1 or args.timeout < 1:
        parser.error("--workers and --timeout must be positive")

    urls = list(read_urls(Path(args.input)))
    matches: list[str] = []
    failures = 0

    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = [pool.submit(fetch_and_scan, url, args.timeout) for url in urls]
        for future in as_completed(futures):
            url, matched, error = future.result()
            if error:
                failures += 1
                if args.show_errors:
                    print(f"scan_failed\t{url}\t{error}", file=sys.stderr)
            elif matched:
                matches.append(url)

    matches.sort()
    output = "".join(f"{url}\n" for url in matches)
    if args.output:
        Path(args.output).write_text(output, encoding="utf-8")
    else:
        sys.stdout.write(output)

    print(f"matched_urls={len(matches)} scanned_urls={len(urls)} failed={failures}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
