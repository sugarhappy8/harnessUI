#!/usr/bin/env python3
"""Download the tiny model the contract tests use (Qwen3 0.6B, Q8_0), pinned by
revision and SHA-256. Prints the path. Standard library only.

  python3 scripts/fetch_test_model.py [dest_dir]
"""
import hashlib
import shutil
import sys
import urllib.request
from pathlib import Path

REPO = "Qwen/Qwen3-0.6B-GGUF"
REVISION = "23749fefcc72300e3a2ad315e1317431b06b590a"
FILE = "Qwen3-0.6B-Q8_0.gguf"
SIZE = 639446688
SHA256 = "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031"


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> None:
    dest_dir = Path(sys.argv[1] if len(sys.argv) > 1 else ".cache/models")
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest = dest_dir / FILE
    if dest.exists() and dest.stat().st_size == SIZE and sha256_of(dest) == SHA256:
        print(dest.resolve())
        return
    url = f"https://huggingface.co/{REPO}/resolve/{REVISION}/{FILE}"
    tmp = dest.with_suffix(".part")
    req = urllib.request.Request(url, headers={"User-Agent": "app-ci/1"})
    with urllib.request.urlopen(req, timeout=300) as resp, tmp.open("wb") as out:
        shutil.copyfileobj(resp, out, length=1 << 20)
    if tmp.stat().st_size != SIZE or sha256_of(tmp) != SHA256:
        tmp.unlink(missing_ok=True)
        raise SystemExit("test model checksum mismatch")
    tmp.replace(dest)
    print(dest.resolve())


if __name__ == "__main__":
    main()
