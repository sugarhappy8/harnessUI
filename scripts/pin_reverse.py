#!/usr/bin/env python3
"""Pins and checks the reverse-engineering downloads (item5-spec.md R1 and WP9a).

Settings -> Reverse engineering downloads rea (the npm tarball), Node.js, Kuna, Ghidra and
Eclipse Temurin on request, like the ROCm pack. `reverse.lock.json` pins every archive by
URL, size and SHA-256, and records what the app checks after unpacking. This script writes
that file and the catalog fixture of the pinned rea, and checks both offline.

Standard library only (plus `gpgv`, `git` and `npm` for the publisher checks). Run with
`python3 -I`. It never runs or extracts what it downloads: archives are listed and single
files are read in memory.

    pin_reverse.py pin --scratch DIR [--rea 6.9.0] [--node 24.22.3] [--java jdk-21.0.13+9]
                       [--ghidra-asset NAME] [--first-download-only ghidra,kuna]
                       [--minimum PART=VERSION --reason TEXT]
    pin_reverse.py --check [--release]
    pin_reverse.py --download-to DIR     (downloads and verifies this computer's parts)
    pin_reverse.py github-digest [--part ghidra|kuna] [--record]
                       [--confirmed SHA256 --size BYTES]

Publisher checks (each one recorded in `verified_by`; the pin stops when one fails or its
verifier is missing):
- rea: npm's sha512 `integrity`; the SLSA provenance names github.com/morluto/rea at the
  commit of tag `rea-agents-<version>` and the tarball's digest; `npm audit signatures`
  in an empty scratch project installed with `--ignore-scripts`.
- Node.js: `SHASUMS256.txt.asc` checked with `gpgv` against `scripts/keys/nodejs.gpg`.
- Temurin: each archive's `.sig` checked with `gpgv` against `scripts/keys/adoptium.gpg`,
  and the checksum Adoptium's API publishes.
- Ghidra and Kuna: the SHA-256 digest GitHub publishes for the release asset (same origin
  as the asset). Where GitHub's API cannot be reached, `--first-download-only <part>`
  records `first-download-only` instead (Kuna always is: its releases carry no signature).
  `github-digest` checks such a pin later against GitHub's digest and size (CI runs it with
  the workflow's read-only `GITHUB_TOKEN`); `--record` writes `github-digest` into the lock,
  online, or offline with `--confirmed <sha256> --size <bytes>` copied from that CI run.

Every part's builds must carry their own publisher's checks (REQUIRED_CHECKS). Ghidra alone
may carry `first-download-only` in place of `github-digest` while 0.0.6 is developed:
`--check` passes and names it, `--check --release` fails, and a release build of the app does
not offer such a part (crates/reverse `lock::release_checks`, `platform::RELEASE_RULES`).

The new lock and fixture are written only after they pass `--check`. A part's minimum
version moves only with `--minimum <part>=<version> --reason <text>` (R15), or for Node.js,
Ghidra and Java with the oldest version rea accepts; a bump keeps the previous rea and Kuna
usable.
"""

from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import json
import os
import platform as host_platform
import re
import shutil
import subprocess
import sys
import tarfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parent.parent
LOCK = ROOT / "reverse.lock.json"
FIXTURES = ROOT / "crates" / "reverse" / "tests" / "fixtures"
KEYS = ROOT / "scripts" / "keys"

SCHEMA = 2
PLATFORMS = ("windows-x86_64", "macos-aarch64", "macos-x86_64", "linux-x86_64")
GITHUB_HOSTS = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"]
HOSTS = {
    "rea": ["registry.npmjs.org"],
    "node": ["nodejs.org"],
    "kuna": GITHUB_HOSTS,
    "ghidra": GITHUB_HOSTS,
    "java": GITHUB_HOSTS,
}
COMPONENT = {"rea": "rea", "node": "rea", "kuna": "kuna", "ghidra": "ghidra", "java": "ghidra"}
# Where each part is offered (the owner's answers Q1 = A and Q2 = B: no Kuna on Windows,
# no Ghidra or Java on a Mac in 0.0.6).
OFFERED = {
    "rea": PLATFORMS,
    "node": PLATFORMS,
    "kuna": ("macos-aarch64", "macos-x86_64", "linux-x86_64"),
    "ghidra": ("windows-x86_64", "linux-x86_64"),
    "java": ("windows-x86_64", "linux-x86_64"),
}
FAMILIES = "direct,enhanced,native,artifact,managed,session,electron,application"
# Ghidra analyzers turned off for every import (R20): a PDB named inside a hostile program
# could point at a network share. "PDB MSDIA" is off by default in 12.1.4 (it is the
# opposite of "PDB Universal"'s default), so naming it is not needed.
DISABLED_GHIDRA_ANALYZERS = {"windows-x86_64": "PDB Universal", "linux-x86_64": "PDB Universal"}
VERIFIED_KINDS = (
    "npm-integrity",
    "npm-provenance:",
    "npm-audit-signatures",
    "gpg:nodejs-release-keys",
    "gpg:adoptium",
    "adoptium-api-checksum",
    "github-digest",
    "first-download-only",
)
# The publisher checks each part's builds must carry for a release (the same table as
# crates/reverse `lock::release_checks`). Kuna's releases have no signature: its first
# download is all there is. Ghidra may carry `first-download-only` instead of
# `github-digest` outside a release (`--check --release` refuses it).
REQUIRED_CHECKS = {
    "rea": ("npm-integrity", "npm-provenance:", "npm-audit-signatures"),
    "node": ("gpg:nodejs-release-keys",),
    "kuna": (),
    "ghidra": ("github-digest",),
    "java": ("gpg:adoptium", "adoptium-api-checksum"),
}
# The tools the app offers (item5-spec.md section 5). The fixture keeps their input schemas.
ALLOW_LISTED = (
    "open_binary", "close_binary", "binary_session",
    "binary_overview", "list_procedures", "search_procedures", "procedure_pseudo_code",
    "procedure_assembly", "procedure_info", "procedure_callers", "procedure_callees",
    "procedure_references", "list_strings", "search_strings", "list_names", "list_segments",
    "xrefs", "find_xrefs_to_name", "get_call_graph", "analyze_function", "batch_decompile",
    "trace_call_path", "trace_feature", "inspect_analysis_view",
    "inspect_pe_imports", "inspect_pe_resources", "inspect_pe_certificates",
    "inspect_native_exports", "inspect_binary_hardening", "inspect_go_binary",
    "inspect_rust_binary", "inspect_managed_artifact", "inspect_managed_members",
    "inspect_python_pyc", "inspect_desktop_application", "inspect_electron_fuses",
    "analyze_javascript_application",
)
APP_ONLY = ("close_binary", "reset_investigation")
LICENSES = {
    "rea": ("MIT", "https://github.com/morluto/rea/blob/main/LICENSE"),
    "node": ("MIT", "https://github.com/nodejs/node/blob/main/LICENSE"),
    "kuna": ("Apache-2.0", "https://github.com/Noelo-Lab/kuna/blob/main/LICENSE"),
    "ghidra": ("Apache-2.0", "https://github.com/NationalSecurityAgency/ghidra/blob/master/LICENSE"),
    "java": ("GPL-2.0 WITH Classpath-exception-2.0", "https://openjdk.org/legal/gplv2+ce.html"),
}
WINDOWS_DEVICE = re.compile(r"^(con|prn|aux|nul|com[0-9]|lpt[0-9])(\..*)?$", re.IGNORECASE)
USER_AGENT = "harnessui-pin-reverse"


def fail(message: str) -> None:
    print(f"pin_reverse: {message}", file=sys.stderr)
    sys.exit(1)


# --------------------------------------------------------------------------------------
# Versions


def version_key(version: str) -> tuple[int, ...]:
    """`21.0.13+9` -> (21, 0, 13, 9); `1.990` -> (1, 990)."""
    parts = re.split(r"[.+_-]", version)
    if not parts or not all(p.isdigit() for p in parts):
        raise ValueError(f"not a version: {version!r}")
    return tuple(int(p) for p in parts)


# --------------------------------------------------------------------------------------
# Network


def http_get(
    url: str, dest: Path | None = None, accept: str | None = None, extra: dict | None = None
) -> bytes:
    """Fetches `url` (https only). With `dest`, streams into that file and returns b''.
    `extra` adds request headers (never printed)."""
    if not url.startswith("https://"):
        fail(f"refusing a non-https URL: {url}")
    headers = {"User-Agent": USER_AGENT}
    if accept:
        headers["Accept"] = accept
    headers.update(extra or {})
    last: Exception | None = None
    for _ in range(3):
        try:
            req = urllib.request.Request(url, headers=headers)
            with urllib.request.urlopen(req, timeout=120) as resp:
                if dest is None:
                    return resp.read()
                tmp = dest.with_name(dest.name + ".tmp")
                with open(tmp, "wb") as out:
                    shutil.copyfileobj(resp, out, 1 << 20)
                tmp.replace(dest)
                return b""
        except urllib.error.HTTPError as e:
            if e.code in (403, 404, 410):
                raise
            last = e
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            last = e
    raise RuntimeError(f"could not fetch {url}: {last}")


def http_json(url: str) -> object:
    return json.loads(http_get(url, accept="application/json"))


def download(url: str, scratch: Path, name: str, size: int | None = None) -> Path:
    dest = scratch / name
    if dest.is_file() and size is not None and dest.stat().st_size == size:
        return dest
    print(f"  downloading {name}")
    http_get(url, dest)
    if size is not None and dest.stat().st_size != size:
        fail(f"{name}: {dest.stat().st_size} bytes, the publisher says {size}")
    return dest


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sha512_b64(path: Path) -> str:
    h = hashlib.sha512()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return base64.b64encode(h.digest()).decode()


def need(program: str) -> str:
    found = shutil.which(program)
    if not found:
        fail(f"{program} is needed for the publisher checks and was not found")
    return found


def https_on(url: object, hosts: list[str]) -> bool:
    """Whether `url` is https on one of `hosts`, with no user, password or port."""
    if not isinstance(url, str):
        return False
    try:
        u = urllib.parse.urlsplit(url)
        port = u.port
    except ValueError:
        return False
    return (
        u.scheme == "https"
        and u.hostname in hosts
        and u.netloc == u.hostname
        and port is None
        and "\\" not in url
    )


def gpgv(keyring: Path, *args: str) -> subprocess.CompletedProcess:
    if not keyring.is_file():
        fail(f"missing key file {keyring}")
    return subprocess.run(
        [need("gpgv"), "--keyring", str(keyring), *args], capture_output=True, text=True
    )


def github_digest(repo: str, tag: str, asset: str) -> str | None:
    """The `sha256:` digest GitHub publishes for a release asset, or None if unreachable."""
    url = f"https://api.github.com/repos/{repo}/releases/tags/{tag}"
    try:
        release = http_json(url)
    except (urllib.error.HTTPError, RuntimeError) as e:
        print(f"  GitHub's release API could not be read ({e})")
        return None
    for a in release.get("assets", []):  # type: ignore[union-attr]
        if a.get("name") == asset:
            digest = a.get("digest") or ""
            if digest.startswith("sha256:"):
                return digest.split(":", 1)[1].lower()
            fail(f"GitHub publishes no SHA-256 digest for {asset}")
    fail(f"{asset} is not an asset of {repo} {tag}")
    return None


def publisher_digest(part: str, repo: str, tag: str, asset: str, path: Path, args) -> list[str]:
    """Checks `path` against GitHub's digest; returns the `verified_by` entries."""
    digest = github_digest(repo, tag, asset)
    got = sha256_file(path)
    if digest is not None:
        if digest != got:
            fail(f"{asset}: SHA-256 {got} differs from GitHub's digest {digest}")
        return ["github-digest"]
    if part in args.first_download_only:
        print(f"  {asset}: trusted on first download (SHA-256 {got})")
        return ["first-download-only"]
    fail(f"{asset}: GitHub's digest is needed (or pass --first-download-only {part})")
    return []


# --------------------------------------------------------------------------------------
# GitHub's digest for a pin recorded as first-download-only (WP9e)
#
# Where GitHub's API cannot be read at pin time, Ghidra is pinned `first-download-only`
# and a release build does not offer it. CI reads the digest GitHub publishes for the
# release asset with the workflow's read-only token (`github-digest --part ghidra`) and
# fails if it differs from the lock's SHA-256 or size; `--record` then writes
# `github-digest` into the lock, online, or offline from the values that run printed
# (`--confirmed <sha256> --size <bytes>`).

DIGEST_REPOS = {"ghidra": "NationalSecurityAgency/ghidra", "kuna": "Noelo-Lab/kuna"}
RELEASE_ASSET_URL = re.compile(
    r"^https://github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/releases/download/([A-Za-z0-9_.+-]+)/([A-Za-z0-9_.+-]+)$"
)


def pinned_assets(lock: dict, name: str) -> list[dict]:
    """The release assets a part's builds pin (one per distinct file), with the repository
    and tag read from the pinned URL, which must be on the part's own repository."""
    part = next((p for p in lock.get("parts", []) if p.get("name") == name), None)
    if part is None or name not in DIGEST_REPOS:
        fail(f"{name}: not a part whose release assets GitHub publishes digests for")
    out: dict[str, dict] = {}
    for b in part.get("builds", []):
        f = b.get("file", {})
        m = RELEASE_ASSET_URL.match(str(f.get("url", "")))
        if not m or m.group(1) != DIGEST_REPOS[name] or m.group(3) != f.get("name"):
            fail(f"{name} {b.get('platform')}: {f.get('url')!r} is not a release asset of {DIGEST_REPOS[name]}")
        a = out.setdefault(f["url"], {
            "part": name, "repo": m.group(1), "tag": m.group(2), "asset": f["name"],
            "url": f["url"], "sha256": f["sha256"], "size": f["size"], "platforms": [],
        })
        if (a["sha256"], a["size"]) != (f["sha256"], f["size"]):
            fail(f"{name}: two builds pin {f['name']} with different digests or sizes")
        a["platforms"].append(b.get("platform"))
    return list(out.values())


def github_release(repo: str, tag: str, token: str | None) -> dict:
    """GitHub's release record for `tag` (the API, with the token when there is one)."""
    url = f"https://api.github.com/repos/{repo}/releases/tags/{urllib.parse.quote(tag, safe='')}"
    extra = {"X-GitHub-Api-Version": "2022-11-28"}
    if token:
        extra["Authorization"] = f"Bearer {token}"
    try:
        data = json.loads(http_get(url, accept="application/vnd.github+json", extra=extra))
    except urllib.error.HTTPError as e:
        fail(f"GitHub's API answered HTTP {e.code} for {repo} {tag}")
    except RuntimeError as e:
        fail(str(e))
    if not isinstance(data, dict):
        fail(f"GitHub's API sent something unexpected for {repo} {tag}")
    return data


def digest_problems(expected: dict, release: dict) -> tuple[str | None, list[str]]:
    """GitHub's SHA-256 for the pinned asset, and how it or the asset differs from the lock."""
    asset = next((a for a in release.get("assets", []) if a.get("name") == expected["asset"]), None)
    if asset is None:
        return None, [f"{expected['asset']} is not an asset of {expected['repo']} {expected['tag']}"]
    digest = str(asset.get("digest") or "")
    problems = []
    got = digest[len("sha256:"):].lower() if digest.startswith("sha256:") else None
    if got is None or not re.fullmatch(r"[0-9a-f]{64}", got):
        problems.append(f"GitHub publishes no SHA-256 digest for {expected['asset']} ({digest!r})")
    elif got != expected["sha256"]:
        problems.append(f"{expected['asset']}: GitHub's digest {got} differs from the lock's {expected['sha256']}")
    if asset.get("size") != expected["size"]:
        problems.append(f"{expected['asset']}: GitHub's size {asset.get('size')} differs from the lock's {expected['size']}")
    if asset.get("state", "uploaded") != "uploaded":
        problems.append(f"{expected['asset']}: GitHub's asset state is {asset.get('state')!r}")
    if asset.get("browser_download_url") != expected["url"]:
        problems.append(f"{expected['asset']}: GitHub's download link {asset.get('browser_download_url')!r} is not the lock's")
    return got, problems


def with_github_digest(lock: dict, name: str) -> dict:
    """A copy of `lock` whose builds of part `name` record `github-digest` (in place of
    `first-download-only`; Kuna keeps it, as `pin` records it: its releases carry no
    signature)."""
    out = copy.deepcopy(lock)
    for p in out["parts"]:
        if p["name"] != name:
            continue
        for b in p["builds"]:
            vb = [v for v in b.get("verified_by", []) if v not in ("github-digest", "first-download-only")]
            vb.insert(0, "github-digest")
            if name == "kuna":
                vb.append("first-download-only")
            b["verified_by"] = vb
    return out


def cmd_github_digest(args) -> None:
    lock = json.loads(LOCK.read_text())
    problems = check(lock)
    if problems:
        fail("reverse.lock.json does not pass --check:\n  " + "\n  ".join(problems))
    assets = pinned_assets(lock, args.part)
    lines = []
    if args.confirmed is not None:
        # Offline: the values a CI run of this command printed, typed in by the lead.
        if len(assets) != 1:
            fail(f"{args.part} pins {len(assets)} assets; --confirmed takes one (run online instead)")
        if not re.fullmatch(r"[0-9a-f]{64}", args.confirmed.lower()) or args.size is None:
            fail("--confirmed needs the 64 hex digits of the SHA-256 and --size the bytes, as CI printed them")
        a = assets[0]
        found = []
        if args.confirmed.lower() != a["sha256"]:
            found.append(f"{a['asset']}: the confirmed digest {args.confirmed.lower()} differs from the lock's {a['sha256']}")
        if args.size != a["size"]:
            found.append(f"{a['asset']}: the confirmed size {args.size} differs from the lock's {a['size']}")
        if found:
            fail("\n".join(found))
        lines.append(f"{args.part}: the confirmed digest sha256:{a['sha256']} and size {a['size']} match {a['asset']} in reverse.lock.json")
    else:
        token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN") or None
        releases: dict[tuple[str, str], dict] = {}
        found = []
        for a in assets:
            key = (a["repo"], a["tag"])
            if key not in releases:
                releases[key] = github_release(a["repo"], a["tag"], token)
            got, wrong = digest_problems(a, releases[key])
            found += wrong
            if not wrong:
                lines.append(
                    f"{args.part}: GitHub publishes sha256:{got} and {a['size']} bytes for {a['asset']} "
                    f"({a['repo']} {a['tag']}, for {', '.join(a['platforms'])}): they match reverse.lock.json"
                )
        if found:
            fail("\n".join(found))
    for line in lines:
        print(line)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write("".join(f"- {line}\n" for line in lines))
    current = [b.get("verified_by") for p in lock["parts"] if p["name"] == args.part for b in p["builds"]]
    if not args.record:
        if any("github-digest" not in (vb or []) for vb in current):
            command = ["python3", "-I", "scripts/pin_reverse.py", "github-digest", "--part", args.part]
            if len(assets) == 1:
                command += ["--confirmed", assets[0]["sha256"], "--size", str(assets[0]["size"])]
            print(
                f"reverse.lock.json records {current[0]} for {args.part}. To record GitHub's digest "
                f"(offline, from these values): {' '.join(command + ['--record'])}"
            )
        return
    new = with_github_digest(lock, args.part)
    problems = check(new)
    if problems:
        fail("the lock with github-digest does not pass --check (nothing was written):\n  " + "\n  ".join(problems))
    LOCK.write_text(json.dumps(new, indent=2) + "\n")
    print(f"wrote {LOCK.relative_to(ROOT)}: {args.part} is verified by GitHub's digest")
    for b in release_blockers(new):
        print(f"NOT READY FOR A RELEASE: {b}")


# --------------------------------------------------------------------------------------
# Archives (listed, never extracted)


def safe_name(rel: str) -> bool:
    if not rel or "\\" in rel or "\0" in rel or ":" in rel or rel.startswith("/"):
        return False
    for p in rel.split("/"):
        if p in ("", ".", "..") or p.endswith((" ", ".")) or WINDOWS_DEVICE.match(p):
            return False
    return True


def survey(path: Path, strip: str, keep: list[str] | str, programs: list[dict]) -> dict:
    """Entries, kept bytes, the longest kept path and each program's size and mode."""
    keep_all = keep == "all"
    want = set() if keep_all else set(keep)
    entries = 0
    kept_bytes = 0
    longest = 0
    seen: dict[str, tuple[int, int]] = {}
    links = 0

    def visit(name: str, size: int, mode: int, kind: str) -> None:
        nonlocal kept_bytes, longest, links
        name = name.rstrip("/")
        if name + "/" == strip or name == strip.rstrip("/"):
            return
        if not name.startswith(strip):
            fail(f"{path.name}: {name!r} is outside {strip!r}")
        rel = name[len(strip):]
        if not safe_name(rel):
            fail(f"{path.name}: unsafe entry name {name!r}")
        if not keep_all and rel not in want:
            return
        longest = max(longest, len(rel))
        if kind == "file":
            kept_bytes += size
            seen[rel] = (size, mode)
        elif kind == "link":
            links += 1

    if path.name.endswith((".tar.gz", ".tgz")):
        with tarfile.open(path, "r:gz") as tf:
            for m in tf:
                if m.type in (tarfile.XGLTYPE, tarfile.XHDTYPE):
                    continue
                entries += 1
                if m.isdir():
                    visit(m.name, 0, m.mode, "dir")
                elif m.isreg():
                    visit(m.name, m.size, m.mode, "file")
                elif m.issym():
                    visit(m.name, 0, m.mode, "link")
                else:
                    fail(f"{path.name}: {m.name!r} is not a file, folder or link")
    elif path.name.endswith(".zip"):
        with zipfile.ZipFile(path) as zf:
            for info in zf.infolist():
                entries += 1
                mode = (info.external_attr >> 16) & 0o177777
                kind = "dir" if info.is_dir() else ("link" if mode & 0o170000 == 0o120000 else "file")
                visit(info.filename, info.file_size, mode, kind)
    else:
        fail(f"unknown archive type: {path.name}")
    if not keep_all:
        missing = want - set(seen)
        if missing:
            fail(f"{path.name}: kept files missing from the archive: {sorted(missing)}")
    recorded = []
    for p in programs:
        if p["path"] not in seen:
            fail(f"{path.name}: program {p['path']} is not in the archive")
        size, mode = seen[p["path"]]
        if p.get("exec") and path.name.endswith((".tar.gz", ".tgz")) and not mode & 0o100:
            fail(f"{path.name}: program {p['path']} is not executable in the archive")
        recorded.append({**p, "size": size})
    return {
        "entries": entries,
        "unpacked_bytes": kept_bytes,
        "max_path_chars": longest,
        "programs": recorded,
        "links": links,
    }


def read_member(path: Path, name: str) -> bytes:
    """One file of a tar.gz, read in memory."""
    with tarfile.open(path, "r:gz") as tf:
        member = tf.getmember(name)
        f = tf.extractfile(member)
        if f is None:
            fail(f"{name} is not a file in {path.name}")
        return f.read()


# --------------------------------------------------------------------------------------
# rea


def js_constant(text: str, name: str) -> str:
    m = re.search(rf"export const {name}\s*=\s*\"?([0-9.]+)\"?;", text)
    if not m:
        fail(f"could not read {name} from the rea package")
    return m.group(1)


def provenance(version: str, tarball_sha512_hex: str) -> str:
    """Checks rea's SLSA provenance; returns the commit it names."""
    tag = f"rea-agents-{version}"
    out = subprocess.run(
        [need("git"), "ls-remote", "https://github.com/morluto/rea", f"refs/tags/{tag}", f"refs/tags/{tag}^{{}}"],
        capture_output=True, text=True, timeout=120,
    )
    lines = [line.split() for line in out.stdout.splitlines() if line.strip()]
    if not lines:
        fail(f"tag {tag} not found in github.com/morluto/rea")
    commits = {sha for sha, _ref in lines}
    att = http_json(f"https://registry.npmjs.org/-/npm/v1/attestations/rea-agents@{version}")
    found = None
    for a in att.get("attestations", []):  # type: ignore[union-attr]
        if a.get("predicateType") != "https://slsa.dev/provenance/v1":
            continue
        envelope = a["bundle"]["dsseEnvelope"]
        statement = json.loads(base64.b64decode(envelope["payload"]))
        subjects = statement.get("subject", [])
        if not any(s.get("digest", {}).get("sha512") == tarball_sha512_hex for s in subjects):
            fail("the provenance does not name the tarball's digest")
        build = statement["predicate"]["buildDefinition"]
        repo = build["externalParameters"]["workflow"]["repository"]
        if repo.rstrip("/") != "https://github.com/morluto/rea":
            fail(f"the provenance names another repository: {repo}")
        deps = build.get("resolvedDependencies", [])
        commit = next((d.get("digest", {}).get("gitCommit") for d in deps if d.get("digest", {}).get("gitCommit")), None)
        if commit not in commits:
            fail(f"the provenance commit {commit} is not tag {tag} ({sorted(commits)})")
        found = commit
    if not found:
        fail("no SLSA provenance found for the rea tarball")
    return found  # type: ignore[return-value]


NPM_REGISTRY = "https://registry.npmjs.org/"


def npm_env(environ: dict[str, str]) -> dict[str, str]:
    """The environment for npm without the developer's npm settings: every `npm_*` and
    `NPM_CONFIG_*` variable (any case) and NODE_OPTIONS are dropped."""
    return {
        k: v for k, v in environ.items()
        if not k.lower().startswith("npm_") and k.upper() != "NODE_OPTIONS"
    }


def npm_isolation_args(user_rc: Path, global_rc: Path) -> list[str]:
    """npm reads no user or global npmrc (two empty files: npm refuses one file for both)
    and asks npm's own registry only."""
    return [f"--registry={NPM_REGISTRY}", f"--userconfig={user_rc}", f"--globalconfig={global_rc}"]


def npm_audit_signatures(version: str, scratch: Path) -> None:
    project = scratch / "npm-audit"
    if project.exists():
        shutil.rmtree(project)
    project.mkdir(parents=True)
    (project / "package.json").write_text('{"name":"pin-check","version":"0.0.0","private":true}\n')
    user_rc, global_rc = scratch / "empty-user-npmrc", scratch / "empty-global-npmrc"
    user_rc.write_text("")
    global_rc.write_text("")
    npm = need("npm")
    env = npm_env(dict(os.environ))
    iso = npm_isolation_args(user_rc, global_rc)
    for cmd in (
        [npm, "install", *iso, "--ignore-scripts", "--no-audit", "--no-fund", f"rea-agents@{version}"],
        [npm, "audit", "signatures", *iso],
    ):
        r = subprocess.run(cmd, cwd=project, capture_output=True, text=True, env=env, timeout=900)
        if r.returncode != 0:
            fail(f"npm {cmd[1]} failed:\n{r.stdout}\n{r.stderr}")
        if cmd[1] == "audit":
            print("  " + r.stdout.strip().replace("\n", "\n  "))


def family_map(tarball: Path, names: set[str]) -> dict[str, str]:
    """Tool name -> rea family, read from the contract files as text (best effort)."""
    files: dict[str, str] = {}
    with tarfile.open(tarball, "r:gz") as tf:
        for m in tf:
            if m.isreg() and m.name.startswith("package/dist/contracts/") and m.name.endswith(".js"):
                f = tf.extractfile(m)
                if f is not None:
                    files[m.name[len("package/dist/contracts/"):]] = f.read().decode("utf-8", "replace")
    families_js = files.get("toolFamilies.js", "")
    imports = dict(
        (group, path.lstrip("./"))
        for group, path in re.findall(r"import \{ (\w+) \} from \"(\./[^\"]+)\";", families_js)
    )
    def group_tools(group: str, path: str, depth: int = 0) -> set[str]:
        """Names in `export const <group> = [...]`, following `...OTHER` spreads."""
        text = files.get(path, "")
        start = text.find(f"export const {group} =")
        if start < 0 or depth > 4:
            return set()
        end = text.find("\n];", start)
        body = text[start:end if end > 0 else len(text)]
        found = {n for n in re.findall(r"\(\s*[\"']([a-z][a-z0-9_]*)[\"']", body) if n in names}
        local = dict(
            (g, str(PurePosixPath(path).parent / p.lstrip("./")).lstrip("./"))
            for g, p in re.findall(r"import \{ (\w+) \} from \"(\./[^\"]+)\";", text)
        )
        for spread in re.findall(r"\.\.\.(\w+_CONTRACTS)\b", body):
            if spread in local:
                found |= group_tools(spread, local[spread], depth + 1)
        return found

    group_names = {group: group_tools(group, path) for group, path in imports.items()}
    out: dict[str, str] = {}
    defs = families_js[families_js.find("TOOL_FAMILY_DEFINITIONS"):]
    for fid, block in re.findall(r"id: \"([a-z-]+)\",(.*?)\n    \}", defs, re.S):
        for group in re.findall(r"TOOL_CONTRACT_GROUPS\.(\w+)", block):
            for n in group_names.get(group, ()):
                out.setdefault(n, fid)
    return out


def pin_rea(version: str, scratch: Path, previous: dict | None) -> tuple[dict, dict]:
    meta = http_json("https://registry.npmjs.org/rea-agents")
    info = meta["versions"].get(version)  # type: ignore[index]
    if not info:
        fail(f"rea-agents {version} is not on npm")
    dist = info["dist"]
    asset = f"rea-agents-{version}.tgz"
    url = dist["tarball"]
    path = download(url, scratch, asset)
    integrity = dist["integrity"]
    if integrity != "sha512-" + sha512_b64(path):
        fail("the rea tarball does not match npm's integrity")
    sha512_hex = hashlib.sha512(path.read_bytes()).hexdigest()
    commit = provenance(version, sha512_hex)
    npm_audit_signatures(version, scratch)
    package = json.loads(read_member(path, "package/package.json"))
    if package.get("version") != version:
        fail("the tarball's package.json names another version")
    kuna_text = read_member(path, "package/dist/kuna/KunaRelease.js").decode()
    ghidra_text = read_member(path, "package/dist/ghidra/GhidraReleasePolicy.js").decode()
    facts = {
        "kuna": js_constant(kuna_text, "KUNA_VERIFIED_VERSION"),
        "ghidra": js_constant(ghidra_text, "SUPPORTED_GHIDRA_VERSION"),
        "java_major": js_constant(ghidra_text, "SUPPORTED_GHIDRA_JAVA_MAJOR"),
        "engines": package.get("engines", {}).get("node", ""),
    }
    gmaj, gmin = version_key(facts["ghidra"])[:2]
    # rea admits the verified line and anything newer on that major (GhidraReleasePolicy).
    facts["ghidra_minimum"] = f"{gmaj}.{gmin}"
    catalog = json.loads(read_member(path, "package/dist/mcp-catalog.json"))
    tools = {t["name"]: t for t in catalog["tools"]}
    families = family_map(path, set(tools))
    missing = [n for n in ALLOW_LISTED + APP_ONLY if n not in tools]
    if missing:
        fail(f"rea {version} lacks allow-listed tools: {missing}")
    fixture = {
        "rea_version": version,
        "source": "package/dist/mcp-catalog.json",
        "families": FAMILIES,
        "tool_count": len(tools),
        "tool_names": sorted(tools),
        "tool_families": {n: families.get(n) for n in sorted(tools)},
        "allow_listed": {n: tools[n]["inputSchema"] for n in ALLOW_LISTED},
        "app_only": {n: tools[n]["inputSchema"] for n in APP_ONLY},
    }
    programs = [
        {"path": "scripts/rea.mjs", "exec": False},
        {"path": "package.json", "exec": False},
        {"path": "native/windows/build/rea-windows-x64.node", "exec": False},
    ]
    s = survey(path, "package/", "all", programs)
    if s["links"]:
        fail("the rea tarball holds links; the app's unpacker refuses them on Windows")
    accepted = [{"version": version, "schema_fixture": f"rea-{version}-catalog.json"}]
    if previous and previous.get("version") != version:
        accepted.append({"version": previous["version"], "schema_fixture": f"rea-{previous['version']}-catalog.json"})
    build = {
        "upstream_asset": asset,
        "file": {"name": asset, "url": url, "size": path.stat().st_size, "sha256": sha256_file(path)},
        "strip_prefix": "package/",
        "keep": "all",
        "programs": s["programs"],
        "max_path_chars": s["max_path_chars"],
        "entries": s["entries"],
        "unpacked_bytes": s["unpacked_bytes"],
        "verified_by": ["npm-integrity", f"npm-provenance:morluto/rea@{commit}", "npm-audit-signatures"],
    }
    part = {
        "name": "rea",
        "entry": "scripts/rea.mjs",
        "accepted_rea_versions": accepted[:2],
        "builds": [{"platform": p, **build} for p in PLATFORMS],
    }
    return part, {"fixture": fixture, "facts": facts}


# --------------------------------------------------------------------------------------
# Node.js

NODE_ASSET = {
    "windows-x86_64": ("win-x64", "zip", ["node.exe", "LICENSE"], "node.exe"),
    "macos-aarch64": ("darwin-arm64", "tar.gz", ["bin/node", "LICENSE"], "bin/node"),
    "macos-x86_64": ("darwin-x64", "tar.gz", ["bin/node", "LICENSE"], "bin/node"),
    "linux-x86_64": ("linux-x64", "tar.gz", ["bin/node", "LICENSE"], "bin/node"),
}


def engines_allow(engines: str, version: str) -> bool:
    """Whether `version` satisfies an `^a.b.c || ^d.e.f || >=g.h.i` range."""
    v = version_key(version)
    for alt in engines.split("||"):
        alt = alt.strip()
        if alt.startswith("^"):
            lo = version_key(alt[1:])
            if v[0] == lo[0] and v >= lo:
                return True
        elif alt.startswith(">="):
            if v >= version_key(alt[2:]):
                return True
    return False


def pin_node(version: str | None, engines: str, scratch: Path) -> dict:
    index = http_json("https://nodejs.org/dist/index.json")
    if version is None:
        lts = [r["version"].lstrip("v") for r in index if r.get("lts")]  # type: ignore[union-attr]
        lts = [v for v in lts if engines_allow(engines, v)]
        if not lts:
            fail(f"no Node.js LTS release satisfies {engines}")
        version = max(lts, key=version_key)
    if not engines_allow(engines, version):
        fail(f"Node.js {version} does not satisfy rea's engines {engines}")
    base = f"https://nodejs.org/dist/v{version}/"
    asc = scratch / f"node-{version}-SHASUMS256.txt.asc"
    http_get(base + "SHASUMS256.txt.asc", asc)
    verified = scratch / f"node-{version}-SHASUMS256.verified.txt"
    r = gpgv(KEYS / "nodejs.gpg", "--output", str(verified), str(asc))
    if r.returncode != 0:
        fail(f"SHASUMS256.txt.asc does not verify against scripts/keys/nodejs.gpg:\n{r.stderr}")
    sums = {}
    for line in verified.read_text().splitlines():
        parts = line.split()
        if len(parts) == 2:
            sums[parts[1]] = parts[0].lower()
    builds = []
    major = version_key(version)[0]
    minimum = next(
        (alt.strip()[1:] for alt in engines.split("||") if alt.strip().startswith("^") and version_key(alt.strip()[1:])[0] == major),
        version,
    )
    for p in PLATFORMS:
        tag, ext, keep, program = NODE_ASSET[p]
        top = f"node-v{version}-{tag}"
        asset = f"{top}.{ext}"
        if asset not in sums:
            fail(f"{asset} is not in Node.js's SHASUMS256.txt")
        path = download(base + asset, scratch, asset)
        if sha256_file(path) != sums[asset]:
            fail(f"{asset} does not match the signed SHASUMS256.txt")
        s = survey(path, top + "/", keep, [{"path": program, "exec": True}])
        builds.append({
            "platform": p,
            "upstream_asset": asset,
            "file": {"name": asset, "url": base + asset, "size": path.stat().st_size, "sha256": sums[asset]},
            "strip_prefix": top + "/",
            "keep": keep,
            "programs": s["programs"],
            "max_path_chars": s["max_path_chars"],
            "entries": s["entries"],
            "unpacked_bytes": s["unpacked_bytes"],
            "verified_by": ["gpg:nodejs-release-keys"],
        })
    return {"name": "node", "version": version, "minimum_version": minimum, "builds": builds}


# --------------------------------------------------------------------------------------
# Kuna, Ghidra, Temurin

KUNA_NAME = {"macos-aarch64": "macos-arm64", "macos-x86_64": "macos-x86_64", "linux-x86_64": "linux-x86_64"}


def pin_kuna(version: str, scratch: Path, args) -> dict:
    builds = []
    for p in OFFERED["kuna"]:
        top = f"kuna-v{version}-{KUNA_NAME[p]}"
        asset = f"{top}.tar.gz"
        url = f"https://github.com/Noelo-Lab/kuna/releases/download/v{version}/{asset}"
        path = download(url, scratch, asset)
        verified = publisher_digest("kuna", "Noelo-Lab/kuna", f"v{version}", asset, path, args)
        if "first-download-only" not in verified:
            verified.append("first-download-only")
        s = survey(path, top + "/", "all", [{"path": "kuna", "exec": True}])
        builds.append({
            "platform": p,
            "upstream_asset": asset,
            "file": {"name": asset, "url": url, "size": path.stat().st_size, "sha256": sha256_file(path)},
            "strip_prefix": top + "/",
            "keep": "all",
            "programs": s["programs"],
            "max_path_chars": s["max_path_chars"],
            "entries": s["entries"],
            "unpacked_bytes": s["unpacked_bytes"],
            "verified_by": verified,
        })
    return {"name": "kuna", "version": version, "minimum_version": version, "builds": builds}


GHIDRA_PROGRAMS = {
    "windows-x86_64": [
        {"path": "support/analyzeHeadless.bat", "exec": False},
        {"path": "Ghidra/Features/Decompiler/os/win_x86_64/decompile.exe", "exec": False},
        {"path": "Ghidra/application.properties", "exec": False},
    ],
    "linux-x86_64": [
        {"path": "support/analyzeHeadless", "exec": True},
        {"path": "Ghidra/Features/Decompiler/os/linux_x86_64/decompile", "exec": True},
        {"path": "Ghidra/application.properties", "exec": False},
    ],
}


def pin_ghidra(version: str, minimum: str, scratch: Path, args) -> dict:
    tag = f"Ghidra_{version}_build"
    asset = args.ghidra_asset
    if not asset:
        try:
            release = http_json(f"https://api.github.com/repos/NationalSecurityAgency/ghidra/releases/tags/{tag}")
            names = [a["name"] for a in release.get("assets", [])]  # type: ignore[union-attr]
            asset = next((n for n in names if re.fullmatch(rf"ghidra_{re.escape(version)}_PUBLIC_\d{{8}}\.zip", n)), None)
        except (urllib.error.HTTPError, RuntimeError):
            asset = None
        if not asset:
            fail("Ghidra's asset name is needed: pass --ghidra-asset ghidra_<version>_PUBLIC_<date>.zip")
    if not re.fullmatch(rf"ghidra_{re.escape(version)}_PUBLIC_\d{{8}}\.zip", asset):
        fail(f"unexpected Ghidra asset name {asset!r}")
    url = f"https://github.com/NationalSecurityAgency/ghidra/releases/download/{tag}/{asset}"
    path = download(url, scratch, asset)
    verified = publisher_digest("ghidra", "NationalSecurityAgency/ghidra", tag, asset, path, args)
    props = None
    with zipfile.ZipFile(path) as zf:
        top = zf.namelist()[0].split("/")[0] + "/"
        props = zf.read(top + "Ghidra/application.properties").decode()
    if f"application.version={version}" not in props:
        fail(f"{asset} is not Ghidra {version}")
    sha = sha256_file(path)
    builds = []
    for p in OFFERED["ghidra"]:
        s = survey(path, top, "all", GHIDRA_PROGRAMS[p])
        if s["links"]:
            fail(f"{asset} holds links; the app's unpacker refuses them on Windows")
        builds.append({
            "platform": p,
            "upstream_asset": asset,
            "file": {"name": asset, "url": url, "size": path.stat().st_size, "sha256": sha},
            "strip_prefix": top,
            "keep": "all",
            "programs": s["programs"],
            "max_path_chars": s["max_path_chars"],
            "entries": s["entries"],
            "unpacked_bytes": s["unpacked_bytes"],
            "verified_by": verified,
        })
    return {"name": "ghidra", "version": version, "minimum_version": minimum, "builds": builds}


JAVA_ASSET = {"windows-x86_64": ("windows", "x64"), "linux-x86_64": ("linux", "x64")}
JAVA_EXT = {"windows": "zip", "linux": "tar.gz"}


def java_asset_name(major: str, version: str, os_name: str, arch: str) -> str:
    """Temurin's own name for a JDK archive: OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz."""
    return f"OpenJDK{major}U-jdk_{arch}_{os_name}_hotspot_{version.replace('+', '_')}.{JAVA_EXT[os_name]}"


def java_package_problems(pkg: dict, expected: str) -> list[str]:
    """What is wrong with a package Adoptium's API describes, checked before anything is
    fetched or written: the name must be exactly Temurin's own (so it is a plain file name
    under --scratch), and both links https on the Java hosts, ending with that name."""
    out = []
    name = pkg.get("name")
    if name != expected or not safe_name(str(name)) or "/" in str(name):
        out.append(f"package name {name!r} is not {expected!r}")
    for key, suffix in (("link", expected), ("signature_link", expected + ".sig")):
        url = pkg.get(key)
        if not https_on(url, HOSTS["java"]) or not str(url).endswith("/" + suffix):
            out.append(f"{key} {url!r} is not https on {HOSTS['java']} ending in {suffix}")
    if not isinstance(pkg.get("size"), int) or pkg["size"] <= 0:
        out.append(f"size {pkg.get('size')!r}")
    if not re.fullmatch(r"[0-9a-fA-F]{64}", str(pkg.get("checksum", ""))):
        out.append(f"checksum {pkg.get('checksum')!r}")
    return out
JAVA_PROGRAMS = {
    "windows-x86_64": [{"path": "bin/java.exe", "exec": False}, {"path": "bin/javac.exe", "exec": False}],
    "linux-x86_64": [{"path": "bin/java", "exec": True}, {"path": "bin/javac", "exec": True}],
}


def pin_java(release: str | None, major: str, scratch: Path) -> dict:
    if release is None:
        latest = http_json(f"https://api.adoptium.net/v3/assets/latest/{major}/hotspot?image_type=jdk&vendor=eclipse")
        names = {b["release_name"] for b in latest}  # type: ignore[union-attr]
        release = max(names, key=lambda n: version_key(n[len("jdk-"):]))
    if not re.fullmatch(rf"jdk-{major}\.\d+\.\d+\+\d+", release):
        fail(f"unexpected Temurin release name {release!r}")
    data = http_json(
        "https://api.adoptium.net/v3/assets/release_name/eclipse/"
        + urllib.request.quote(release)
        + "?image_type=jdk&project=jdk&heap_size=normal&vendor=eclipse"
    )
    version = release[len("jdk-"):]
    builds = []
    for p in OFFERED["java"]:
        os_name, arch = JAVA_ASSET[p]
        b = next(
            (b for b in data["binaries"]  # type: ignore[index]
             if b["os"] == os_name and b["architecture"] == arch and b["image_type"] == "jdk"),
            None,
        )
        if b is None:
            fail(f"Temurin {release} has no JDK for {p}")
        pkg = b["package"]
        problems = java_package_problems(pkg, java_asset_name(major, version, os_name, arch))
        if problems:
            fail(f"Adoptium's answer for {p} is not what was expected: " + "; ".join(problems))
        path = download(pkg["link"], scratch, pkg["name"], pkg["size"])
        sig = scratch / (pkg["name"] + ".sig")
        http_get(pkg["signature_link"], sig)
        r = gpgv(KEYS / "adoptium.gpg", str(sig), str(path))
        if r.returncode != 0:
            fail(f"{pkg['name']}.sig does not verify against scripts/keys/adoptium.gpg:\n{r.stderr}")
        sha = sha256_file(path)
        if sha != pkg["checksum"].lower():
            fail(f"{pkg['name']}: SHA-256 {sha} differs from Adoptium's {pkg['checksum']}")
        s = survey(path, release + "/", "all", JAVA_PROGRAMS[p])
        if os_name == "windows" and s["links"]:
            fail(f"{pkg['name']} holds links; the app's unpacker refuses them on Windows")
        builds.append({
            "platform": p,
            "upstream_asset": pkg["name"],
            "file": {"name": pkg["name"], "url": pkg["link"], "size": pkg["size"], "sha256": sha},
            "strip_prefix": release + "/",
            "keep": "all",
            "programs": s["programs"],
            "max_path_chars": s["max_path_chars"],
            "entries": s["entries"],
            "unpacked_bytes": s["unpacked_bytes"],
            "java_home": "",
            "verified_by": ["gpg:adoptium", "adoptium-api-checksum"],
        })
    return {"name": "java", "version": version, "minimum_version": major, "vendor": "Eclipse Temurin", "builds": builds}


# --------------------------------------------------------------------------------------
# Writing and checking the lock


def finish_part(part: dict, version: str, minimum: str) -> dict:
    name = part["name"]
    license_name, license_url = LICENSES[name]
    out = {
        "name": name,
        "component": COMPONENT[name],
        "version": version,
        "minimum_version": minimum,
        "dir": f"{name}-{version}",
        "license_name": license_name,
        "license_url": license_url,
        "hosts": HOSTS[name],
    }
    for key in ("vendor", "entry", "accepted_rea_versions", "builds"):
        if key in part:
            out[key] = part[key]
    return out


def diff_report(old: dict | None, new: dict) -> None:
    print("\nFor the reviewer:")
    if old is None:
        print(f"  first fixture for rea {new['rea_version']} ({new['tool_count']} tools)")
    else:
        added = sorted(set(new["tool_names"]) - set(old["tool_names"]))
        removed = sorted(set(old["tool_names"]) - set(new["tool_names"]))
        print(f"  tools added since rea {old['rea_version']}: {added or 'none'}")
        print(f"  tools removed: {removed or 'none'}")
        for n in ALLOW_LISTED:
            if old["allow_listed"].get(n) != new["allow_listed"].get(n):
                print(f"  allow-listed schema changed: {n}")
    kept = set(FAMILIES.split(","))
    fam = new["tool_families"]
    unknown = sorted(n for n, f in fam.items() if f is None)
    for n in ALLOW_LISTED:
        if fam.get(n) not in kept:
            print(f"  WARNING: allow-listed {n} is in family {fam.get(n)}, not enabled by REA_MCP_TOOL_FAMILIES")
    enabled = sorted(n for n, f in fam.items() if f in kept and n not in ALLOW_LISTED and n not in APP_ONLY)
    print(f"  not offered but enabled inside rea under the kept families ({len(enabled)}):")
    for i in range(0, len(enabled), 6):
        print("    " + ", ".join(enabled[i:i + 6]))
    if unknown:
        print(f"  family not found for {len(unknown)} tools: {', '.join(unknown)}")


def apply_minimums(parts: list[dict], minimums: list[str], reason: str | None, old: dict | None) -> None:
    old_parts = {p["name"]: p for p in (old or {}).get("parts", [])}
    for part in parts:
        prev = old_parts.get(part["name"])
        if prev and version_key(prev["minimum_version"]) >= version_key(part["minimum_version"]):
            part["minimum_version"] = prev["minimum_version"]
            if prev.get("minimum_reason"):
                part["minimum_reason"] = prev["minimum_reason"]
    for item in minimums:
        name, _, ver = item.partition("=")
        part = next((p for p in parts if p["name"] == name), None)
        if part is None or not ver:
            fail(f"--minimum {item}: unknown part or version")
        if not reason:
            fail("--minimum needs --reason")
        if version_key(ver) > version_key(part["version"]):
            fail(f"--minimum {item} is above the pinned {part['version']}")
        part["minimum_version"] = ver
        part["minimum_reason"] = reason


def carried_minimum(name: str, version: str, old: dict | None) -> str:
    """rea and Kuna: the previous lock's minimum, so a bump keeps the previous version usable
    with an update offered (R15); the pin itself on a first pin. Only `--minimum` with
    `--reason` raises it."""
    prev = next((p for p in (old or {}).get("parts", []) if p.get("name") == name), None)
    if prev:
        try:
            if version_key(prev["minimum_version"]) <= version_key(version):
                return prev["minimum_version"]
        except (KeyError, ValueError):
            pass
    return version


def assemble_parts(raw: dict, rea_version: str, old: dict | None, minimums: list[str], reason: str | None) -> list[dict]:
    """The lock's parts from what each pin_* returned, with their minimum versions."""
    rea, node, kuna, ghidra, java = (raw[n] for n in ("rea", "node", "kuna", "ghidra", "java"))
    parts = [
        finish_part(rea, rea_version, carried_minimum("rea", rea_version, old)),
        finish_part(node, node["version"], node["minimum_version"]),
        finish_part(kuna, kuna["version"], carried_minimum("kuna", kuna["version"], old)),
        finish_part(ghidra, ghidra["version"], ghidra["minimum_version"]),
        finish_part(java, java["version"], java["minimum_version"]),
    ]
    apply_minimums(parts, minimums, reason, old)
    return parts


def cmd_pin(args) -> None:
    scratch = Path(args.scratch).resolve()
    if scratch == ROOT or ROOT in scratch.parents:
        fail("--scratch must be outside the repository")
    scratch.mkdir(parents=True, exist_ok=True)
    old = json.loads(LOCK.read_text()) if LOCK.is_file() else None
    old_rea = next((p for p in (old or {}).get("parts", []) if p["name"] == "rea"), None)
    print(f"rea {args.rea}")
    rea, extra = pin_rea(args.rea, scratch, old_rea)
    facts = extra["facts"]
    print(f"  rea verifies Kuna {facts['kuna']}, Ghidra {facts['ghidra']} with JDK {facts['java_major']}; Node {facts['engines']}")
    print("Node.js")
    node = pin_node(args.node, facts["engines"], scratch)
    print(f"Kuna {facts['kuna']}")
    kuna = pin_kuna(facts["kuna"], scratch, args)
    print(f"Ghidra {facts['ghidra']}")
    ghidra = pin_ghidra(facts["ghidra"], facts["ghidra_minimum"], scratch, args)
    print("Temurin")
    java = pin_java(args.java, facts["java_major"], scratch)
    raw = {"rea": rea, "node": node, "kuna": kuna, "ghidra": ghidra, "java": java}
    parts = assemble_parts(raw, args.rea, old, args.minimum or [], args.reason)
    lock = {
        "schema": SCHEMA,
        "comment": (
            "Pinned downloads for Settings -> Reverse engineering (item5-spec.md R1). Written by "
            "scripts/pin_reverse.py and compiled into the app (crates/reverse). Each archive is "
            "checked against its size and SHA-256 before it is unpacked. Not part of the installer."
        ),
        "families": FAMILIES,
        "disabled_ghidra_analyzers": DISABLED_GHIDRA_ANALYZERS,
        "parts": parts,
    }
    fixture_path = FIXTURES / f"rea-{args.rea}-catalog.json"
    old_fixture = None
    if old_rea and old_rea.get("version") != args.rea:
        prev = FIXTURES / f"rea-{old_rea['version']}-catalog.json"
        if prev.is_file():
            old_fixture = json.loads(prev.read_text())
    diff_report(old_fixture, extra["fixture"])
    # Checked before anything is written: a lock that fails leaves the tree as it was.
    problems = check(lock, fixtures={fixture_path.name: extra["fixture"]})
    if problems:
        fail("the new lock does not pass --check (nothing was written):\n  " + "\n  ".join(problems))
    FIXTURES.mkdir(parents=True, exist_ok=True)
    fixture_path.write_text(json.dumps(extra["fixture"], indent=1, sort_keys=False) + "\n")
    LOCK.write_text(json.dumps(lock, indent=2) + "\n")
    print(f"\nwrote {LOCK.relative_to(ROOT)} and {fixture_path.relative_to(ROOT)}")
    for b in release_blockers(lock):
        print(f"NOT READY FOR A RELEASE: {b}")


def has_check(verified_by: list, kind: str) -> bool:
    """Whether `kind` was recorded (`npm-provenance:` needs a commit after the colon)."""
    if kind.endswith(":"):
        return any(isinstance(v, str) and v.startswith(kind) and len(v) > len(kind) for v in verified_by)
    return kind in verified_by


def release_ready(name: str, verified_by: list) -> bool:
    """Whether a build of part `name` carries every check a release needs."""
    need = REQUIRED_CHECKS.get(name)
    return need is not None and bool(verified_by) and all(has_check(verified_by, k) for k in need)


def release_blockers(lock: dict) -> list[str]:
    """Builds a release build of the app would not offer (their publisher check is missing)."""
    return [
        f"{p.get('name')} {b.get('platform')}: verified by {b.get('verified_by')}, "
        f"a release needs {list(REQUIRED_CHECKS.get(p.get('name'), ('?',)))}"
        for p in lock.get("parts", [])
        for b in p.get("builds", [])
        if not release_ready(p.get("name"), b.get("verified_by") or [])
    ]


def check(lock: dict, fixtures: dict | None = None, release: bool = False) -> list[str]:
    """Offline checks of the lock and its fixtures; returns the problems found.

    `fixtures` holds fixtures not yet written (by name); others are read from disk. With
    `release`, every build must also carry its part's publisher checks (no
    `first-download-only` Ghidra)."""
    out: list[str] = []
    if lock.get("schema") != SCHEMA:
        out.append(f"schema is {lock.get('schema')}, expected {SCHEMA}")
    if lock.get("families") != FAMILIES:
        out.append("families differ from the allow-list's families")
    parts = {p.get("name"): p for p in lock.get("parts", [])}
    if set(parts) != set(HOSTS):
        out.append(f"parts are {sorted(parts)}, expected {sorted(HOSTS)}")
    for name, part in parts.items():
        where = f"part {name}"
        try:
            v = version_key(part["version"])
            m = version_key(part["minimum_version"])
            if m > v:
                out.append(f"{where}: minimum_version {part['minimum_version']} is above version")
        except (KeyError, ValueError) as e:
            out.append(f"{where}: bad version fields ({e})")
            continue
        if part.get("component") != COMPONENT.get(name):
            out.append(f"{where}: component {part.get('component')}")
        if part.get("dir") != f"{name}-{part['version']}" or not safe_name(part.get("dir", "")):
            out.append(f"{where}: dir {part.get('dir')!r}")
        if part.get("hosts") != HOSTS.get(name):
            out.append(f"{where}: hosts {part.get('hosts')}")
        if not part.get("license_name") or not str(part.get("license_url", "")).startswith("https://"):
            out.append(f"{where}: licence")
        platforms = [b.get("platform") for b in part.get("builds", [])]
        if sorted(platforms) != sorted(OFFERED[name]):
            out.append(f"{where}: platforms {platforms}, expected {list(OFFERED[name])}")
        for b in part.get("builds", []):
            bw = f"{where} {b.get('platform')}"
            f = b.get("file", {})
            url = str(f.get("url", ""))
            host = re.match(r"^https://([^/:@]+)/", url)
            if not host or host.group(1) not in part.get("hosts", []):
                out.append(f"{bw}: url {url!r} is not https on the part's hosts")
            if not re.fullmatch(r"[0-9a-f]{64}", str(f.get("sha256", ""))):
                out.append(f"{bw}: sha256")
            if not isinstance(f.get("size"), int) or f["size"] <= 0:
                out.append(f"{bw}: size")
            fname = str(f.get("name") or "")
            if (
                not fname or "/" in fname or not safe_name(fname)
                or not url.endswith("/" + fname) or fname != b.get("upstream_asset")
            ):
                out.append(f"{bw}: file name {fname!r}")
            strip = b.get("strip_prefix", "")
            if not strip.endswith("/") or not safe_name(strip.rstrip("/")):
                out.append(f"{bw}: strip_prefix {strip!r}")
            keep = b.get("keep")
            if keep != "all" and (not isinstance(keep, list) or not keep or not all(safe_name(k) for k in keep)):
                out.append(f"{bw}: keep")
            for p in b.get("programs", []):
                if not safe_name(p.get("path", "")) or not isinstance(p.get("size"), int):
                    out.append(f"{bw}: program {p}")
                elif keep != "all" and p["path"] not in keep:
                    out.append(f"{bw}: program {p['path']} is not kept")
            if not b.get("programs"):
                out.append(f"{bw}: no programs")
            for k in ("max_path_chars", "entries", "unpacked_bytes"):
                if not isinstance(b.get(k), int) or b[k] <= 0:
                    out.append(f"{bw}: {k}")
            vb = b.get("verified_by") or []
            if not vb or not all(any(x == k or (k.endswith(":") and x.startswith(k)) for k in VERIFIED_KINDS) for x in vb):
                out.append(f"{bw}: verified_by {vb}")
            elif not release_ready(name, vb):
                waiting = name == "ghidra" and vb == ["first-download-only"] and not release
                if not waiting:
                    need = [k for k in REQUIRED_CHECKS.get(name, ("?",)) if not has_check(vb, k)]
                    out.append(f"{bw}: verified_by {vb} lacks {need}")
    rea = parts.get("rea", {})
    if not rea.get("entry") or not safe_name(rea.get("entry", "")):
        out.append("rea: entry")
    accepted = rea.get("accepted_rea_versions", [])
    if not accepted or len(accepted) > 2 or accepted[0].get("version") != rea.get("version"):
        out.append("rea: accepted_rea_versions must start with the pinned version and hold at most two")
    for a in accepted:
        try:
            if version_key(a["version"]) < version_key(rea["minimum_version"]) and not rea.get("minimum_reason"):
                out.append(
                    f"rea: accepted version {a['version']} is below minimum_version "
                    f"{rea['minimum_version']} with no minimum_reason"
                )
        except (KeyError, ValueError):
            out.append(f"rea: accepted version {a.get('version')!r}")
        name = str(a.get("schema_fixture", ""))
        given = (fixtures or {}).get(name)
        fixture = FIXTURES / name
        plain = safe_name(name) and "/" not in name
        if name != f"rea-{a.get('version')}-catalog.json" or not plain or (given is None and not fixture.is_file()):
            out.append(f"rea: fixture for {a.get('version')} missing")
            continue
        data = given if given is not None else json.loads(fixture.read_text())
        if data.get("rea_version") != a.get("version"):
            out.append(f"rea: fixture {name} names another version")
        missing = [n for n in ALLOW_LISTED if n not in data.get("allow_listed", {})]
        if missing:
            out.append(f"rea: fixture {name} lacks {missing}")
        if data.get("tool_count") != len(data.get("tool_names", [])):
            out.append(f"rea: fixture {name} tool count")
    analyzers = lock.get("disabled_ghidra_analyzers", {})
    for p in OFFERED["ghidra"]:
        names = [n.strip() for n in str(analyzers.get(p, "")).split(",") if n.strip()]
        if not names or not all(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9 ._-]*", n) for n in names):
            out.append(f"disabled_ghidra_analyzers for {p}")
    return out


def current_platform() -> str:
    system = {"Windows": "windows", "Darwin": "macos", "Linux": "linux"}.get(host_platform.system(), "?")
    machine = host_platform.machine().lower()
    arch = {"amd64": "x86_64", "x86_64": "x86_64", "arm64": "aarch64", "aarch64": "aarch64"}.get(machine, machine)
    return f"{system}-{arch}"


def cmd_download_to(target: str) -> None:
    lock = json.loads(LOCK.read_text())
    problems = check(lock)
    if problems:
        fail("the lock does not pass --check:\n  " + "\n  ".join(problems))
    plat = current_platform()
    dest = Path(target).resolve()
    dest.mkdir(parents=True, exist_ok=True)
    for part in lock["parts"]:
        build = next((b for b in part["builds"] if b["platform"] == plat), None)
        if build is None:
            print(f"{part['name']}: not offered on {plat}")
            continue
        f = build["file"]
        path = dest / f["name"]
        if not (path.is_file() and path.stat().st_size == f["size"] and sha256_file(path) == f["sha256"]):
            path = download(f["url"], dest, f["name"], f["size"])
        if sha256_file(path) != f["sha256"]:
            path.unlink()
            fail(f"{f['name']} does not match its pinned SHA-256")
        print(f"{part['name']}: {f['name']} verified")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="check the lock and fixtures offline")
    parser.add_argument(
        "--release", action="store_true",
        help="with --check: also refuse builds without their publisher's checks (no first-download-only Ghidra)",
    )
    parser.add_argument("--download-to", metavar="DIR", help="download and verify this computer's parts")
    sub = parser.add_subparsers(dest="command")
    gd = sub.add_parser(
        "github-digest",
        help="compare GitHub's published digest of a pinned release asset with the lock (and record it)",
    )
    gd.add_argument("--part", choices=sorted(DIGEST_REPOS), default="ghidra")
    gd.add_argument("--record", action="store_true", help="write github-digest into the lock once it matches")
    gd.add_argument("--confirmed", metavar="SHA256", help="offline: the digest a CI run of this command printed")
    gd.add_argument("--size", type=int, help="with --confirmed: the size in bytes that run printed")
    pin = sub.add_parser("pin", help="pin the parts and write the lock and the fixture")
    pin.add_argument("--scratch", required=True, help="download folder (outside the repository)")
    pin.add_argument("--rea", default="6.9.0")
    pin.add_argument("--node", default=None, help="Node.js version (default: newest LTS rea accepts)")
    pin.add_argument("--java", default=None, help="Temurin release name, e.g. jdk-21.0.13+9")
    pin.add_argument("--ghidra-asset", default=None, help="Ghidra's release asset name")
    pin.add_argument("--first-download-only", default="", help="parts trusted on first download (comma list)")
    pin.add_argument("--minimum", action="append", help="PART=VERSION, raised for a security fix")
    pin.add_argument("--reason", default=None, help="why the minimum was raised")
    args = parser.parse_args()
    if args.check:
        if not LOCK.is_file():
            fail("reverse.lock.json is missing")
        lock = json.loads(LOCK.read_text())
        problems = check(lock, release=args.release)
        if problems:
            fail("reverse.lock.json:\n  " + "\n  ".join(problems))
        blockers = release_blockers(lock)
        for b in blockers:
            print(f"not ready for a release (a release build does not offer it): {b}")
        if args.release:
            print("reverse.lock.json and its fixtures pass, ready for a release")
        else:
            print("reverse.lock.json and its fixtures pass" + (" (not ready for a release)" if blockers else ""))
    elif args.download_to:
        cmd_download_to(args.download_to)
    elif args.command == "github-digest":
        cmd_github_digest(args)
    elif args.command == "pin":
        args.first_download_only = {p.strip() for p in args.first_download_only.split(",") if p.strip()}
        cmd_pin(args)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
