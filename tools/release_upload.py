#!/usr/bin/env python3
"""Resume bounded draft uploads; publication requires the workflow's full readback."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time

MAX_ASSETS = 64
WRITE_INTERVAL = 1.0
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
QUERY = """query($owner:String!,$name:String!,$tag:String!){
  repository(owner:$owner,name:$name){
    id
    release(tagName:$tag){databaseId isDraft tagName}
  }
}"""


def require(condition, message):
    if not condition:
        raise ValueError(message)


def local_assets(paths):
    require(0 < len(paths) <= MAX_ASSETS, "expected 1..64 local release assets")
    assets = {}
    for value in paths:
        path = Path(value)
        require(NAME.fullmatch(path.name) is not None, "unsafe local asset name")
        require(path.name not in assets, "duplicate local asset name: " + path.name)
        require(not path.is_symlink() and path.is_file(), "asset is not a regular file")
        path = path.resolve()
        size = path.stat().st_size
        require(size > 0, "standalone release assets must be nonempty")
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        assets[path.name] = {"path": path, "size": size, "digest": "sha256:" + digest}
    return assets


def command(arguments, runner):
    result = runner(["gh", *arguments], capture_output=True, text=True,
                    check=False, timeout=300)
    if result.returncode:
        raise RuntimeError(f"GitHub CLI failed (exit {result.returncode}): "
                           + result.stderr.strip()[-2000:])
    return result.stdout


def document(arguments, runner):
    try:
        return json.loads(command(arguments, runner))
    except json.JSONDecodeError as error:
        raise ValueError("GitHub returned invalid JSON") from error


def release_identity(repo, tag, runner):
    owner, name = repo.split("/")
    response = document(["api", "graphql", "-f", "query=" + QUERY,
                         "-f", "owner=" + owner, "-f", "name=" + name,
                         "-f", "tag=" + tag], runner)
    require(isinstance(response, dict) and not response.get("errors"),
            "GraphQL release lookup failed")
    data = response.get("data")
    repository = data.get("repository") if isinstance(data, dict) else None
    require(isinstance(repository, dict) and isinstance(repository.get("id"), str)
            and repository["id"], "repository lookup failed")
    require("release" in repository, "GraphQL release result is missing")
    release = repository["release"]
    if release is None:
        return None
    require(isinstance(release, dict)
            and type(release.get("databaseId")) is int and release["databaseId"] > 0
            and type(release.get("isDraft")) is bool and release.get("tagName") == tag,
            "invalid release identity")
    return release


def remote_assets(repo, release, local, runner):
    # A 100-item page is sufficient to reject every release above our 64-item bound.
    values = document(["api", f"repos/{repo}/releases/{release['databaseId']}/assets?per_page=100"],
                      runner)
    require(isinstance(values, list) and len(values) <= MAX_ASSETS,
            "release has too many assets or an invalid asset list")
    assets = {}
    for asset in values:
        require(isinstance(asset, dict), "invalid release asset")
        name = asset.get("name")
        require(isinstance(name, str) and NAME.fullmatch(name) is not None,
                "unsafe remote asset name")
        require(name not in assets, "duplicate remote asset name: " + name)
        require(asset.get("state") == "uploaded",
                "release asset is not fully uploaded: " + name)
        require(type(asset.get("size")) is int and asset["size"] > 0,
                "invalid remote asset size: " + name)
        digest = asset.get("digest")
        require(digest is None or (isinstance(digest, str)
                and re.fullmatch(r"sha256:[0-9a-f]{64}", digest) is not None),
                "invalid remote asset digest: " + name)
        if name in local:
            require(asset["size"] == local[name]["size"],
                    "existing release asset size differs: " + name)
            require(digest is None or digest == local[name]["digest"],
                    "existing release asset digest differs: " + name)
        assets[name] = asset
    require(len(assets.keys() | local.keys()) <= MAX_ASSETS,
            "combined release asset set exceeds 64")
    return assets


def upload(repo, tag, notes, paths, *, runner=subprocess.run, sleeper=time.sleep):
    require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9][A-Za-z0-9._-]*", repo)
            is not None, "expected GitHub owner/repository")
    require(re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag) is not None,
            "expected stable vX.Y.Z release tag")
    local = local_assets(paths)

    def write(arguments):
        try:
            command(arguments, runner)
        finally:
            # Also pause after a failed write, then propagate the failure without retry.
            sleeper(WRITE_INTERVAL)

    release = release_identity(repo, tag, runner)
    created = release is None
    if created:
        write(["release", "create", tag, "--repo", repo, "--verify-tag",
               "--generate-notes", "--draft", "--notes", notes])
        release = release_identity(repo, tag, runner)
        require(release is not None and release["isDraft"],
                "new draft was not confirmed")
    remote = remote_assets(repo, release, local, runner)
    missing = [name for name in local if name not in remote]
    uploaded = []
    if release["isDraft"]:
        for name in missing:
            asset = local[name]
            current = local_assets([asset["path"]])[name]
            require(current["size"] == asset["size"] and current["digest"] == asset["digest"],
                    "local asset changed before upload: " + name)
            write(["release", "upload", tag, "--repo", repo, str(asset["path"])])
            uploaded.append(name)
    return {"release_id": release["databaseId"], "tag": tag, "draft": release["isDraft"],
            "created": created, "uploaded": uploaded,
            "existing": [name for name in local if name in remote],
            "missing_from_published_release": missing if not release["isDraft"] else [],
            "readback_required": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--notes", required=True)
    parser.add_argument("files", nargs="+", type=Path)
    args = parser.parse_args()
    try:
        report = upload(args.repo, args.tag, args.notes, args.files)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        parser.exit(1, f"release upload stopped: {error}\n")
    print(json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    main()
