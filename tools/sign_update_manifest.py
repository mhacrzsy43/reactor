#!/usr/bin/env python3
"""Build and Ed25519-sign a Reactor update manifest without exposing the private key."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path


def compact_json(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact", type=Path, required=True)
    parser.add_argument("--artifact-url", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--channel", choices=("stable", "beta"), default="stable")
    parser.add_argument("--private-key", type=Path, required=True)
    parser.add_argument("--public-key-base64", required=True)
    parser.add_argument("--key-id", required=True)
    parser.add_argument("--database-schema", type=int, default=2)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    if not args.artifact.is_file() or not args.private_key.is_file():
        raise SystemExit("artifact or private key does not exist")
    if not args.artifact_url.startswith("https://"):
        raise SystemExit("artifact URL must use HTTPS")
    public_der = subprocess.check_output(
        ["openssl", "pkey", "-in", str(args.private_key), "-pubout", "-outform", "DER"]
    )
    derived_public = public_der[-32:]
    configured_public = base64.b64decode(args.public_key_base64, validate=True)
    if len(configured_public) != 32 or configured_public != derived_public:
        raise SystemExit("configured release public key does not match the private signing key")

    artifact_bytes = args.artifact.read_bytes()
    compatibility = {
        "minimumAppVersion": "0.1.0",
        "databaseSchema": args.database_schema,
        "flowSchemas": [1],
        "resultSchemas": [1],
    }
    artifacts = [{
        "platform": args.platform,
        "arch": args.arch,
        "url": args.artifact_url,
        "sha256": hashlib.sha256(artifact_bytes).hexdigest(),
        "size": len(artifact_bytes),
    }]
    published_at = datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    payload = {
        "schemaVersion": 1,
        "channel": args.channel,
        "version": args.version,
        "publishedAt": published_at,
        "compatibility": compatibility,
        "artifacts": artifacts,
        "signatureAlgorithm": "Ed25519",
        "signatureKeyId": args.key_id,
    }
    with tempfile.TemporaryDirectory(prefix="reactor-sign-") as temporary:
        payload_path = Path(temporary) / "payload.json"
        signature_path = Path(temporary) / "signature.bin"
        payload_path.write_bytes(compact_json(payload))
        subprocess.run(
            [
                "openssl", "pkeyutl", "-sign", "-rawin",
                "-inkey", str(args.private_key),
                "-in", str(payload_path), "-out", str(signature_path),
            ],
            check=True,
        )
        signature = base64.b64encode(signature_path.read_bytes()).decode("ascii")

    manifest = {
        "schemaVersion": 1,
        "channel": args.channel,
        "version": args.version,
        "publishedAt": published_at,
        "compatibility": compatibility,
        "artifacts": artifacts,
        "signature": {
            "algorithm": "Ed25519",
            "keyId": args.key_id,
            "value": signature,
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
