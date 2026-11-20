#!/usr/bin/env python3
"""Compare fixed Git trees with one explicit production-source counting policy."""
import argparse
from collections import Counter, defaultdict
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess

EXTENSIONS = {".rs", ".c", ".h"}
EXCLUDED_DIRS = {
    "test", "tests", "example", "examples", "fixture", "fixtures", "bench", "benches",
    "doc", "docs", "target", "artifacts", "generated", "generated_src", "generated-src",
    "out", "__pycache__", ".git",
}
TEST_FILE = re.compile(r"^(?:tests?|tests?_.*|.*_tests?)\.(?:rs|c|h)$", re.I)
GENERATED_FILE = re.compile(r"(?:[._]generated\.(?:rs|c|h)$|^generated[._].*\.(?:rs|c|h)$)", re.I)
INLINE_MARKER = re.compile(rb"#\s*\[\s*(?:test\b|cfg(?:_attr)?\s*\([^]]*\btest\b)")
ENV = dict(os.environ, GIT_OPTIONAL_LOCKS="0", GIT_NO_LAZY_FETCH="1")
RULES = {
    "production_roots_per_repository": ["src/**", "native/**", "build.rs", "crates/**"],
    "extensions": sorted(EXTENSIONS),
    "excluded_directory_components_case_insensitive": sorted(EXCLUDED_DIRS),
    "excluded_test_filename_regex": TEST_FILE.pattern,
    "excluded_generated_filename_regex": GENERATED_FILE.pattern,
    "line_definition": "LF byte count plus one for a nonempty final unterminated line",
    "blank_lines_and_comments": "included",
    "inline_tests": "included in full file length; text markers are disclosure only, not a parser",
    "gitlinks": "recursively expanded only inside production roots; count each mounted occurrence",
    "registry_system_dependencies": "not traversed; only versioned blobs and recursive gitlinks",
    "symlinks": "excluded; never followed as source",
    "missing_gitlink_objects": "fail without fetching, cloning, or initializing repositories",
}


class GitSource:
    def __init__(self, worktree=None, gitdir=None):
        self.worktree = Path(worktree).resolve() if worktree else None
        if gitdir is None:
            gitdir = subprocess.check_output(
                ["git", "-C", str(self.worktree), "rev-parse", "--absolute-git-dir"],
                env=ENV, text=True).strip()
        self.gitdir = Path(gitdir).resolve()
        self.common = Path(self.run("rev-parse", "--path-format=absolute",
                                    "--git-common-dir").decode().strip()).resolve()

    def run(self, *args, check=True, data=None):
        result = subprocess.run(["git", "--git-dir", str(self.gitdir), *args],
                                env=ENV, input=data, capture_output=True)
        if check and result.returncode:
            raise RuntimeError(result.stderr.decode(errors="replace").strip())
        return result.stdout

    def has_commit(self, sha):
        return self.run("cat-file", "-t", sha, check=False).strip() == b"commit"

    def tree(self, sha):
        entries = []
        for row in self.run("ls-tree", "-r", "-z", sha).split(b"\0"):
            if row:
                metadata, name = row.split(b"\t", 1)
                mode, kind, oid = metadata.decode().split()
                entries.append({"path": name.decode(), "mode": mode, "type": kind, "blob": oid})
        return entries

    def modules(self, sha):
        values = defaultdict(dict)
        raw = self.run("config", "--null", "--blob", sha + ":.gitmodules",
                       "--get-regexp", r"^submodule\..*\.(path|url)$", check=False)
        for item in raw.split(b"\0"):
            if item:
                key, value = item.decode().split("\n", 1)
                match = re.fullmatch(r"submodule\.(.*)\.(path|url)", key)
                if match:
                    values[match[1]][match[2]] = value
        return {value["path"]: {"name": name, **value}
                for name, value in values.items() if "path" in value}

    def child(self, path, sha, module):
        attempts = []
        working = self.worktree / path if self.worktree else None
        if working and (working / ".git").exists():
            attempts.append(str(working))
            candidate = GitSource(worktree=working)
            if candidate.has_commit(sha):
                return candidate
        names = {path, module.get("name", path)}
        for base in (self.gitdir, self.common):
            for name in sorted(names):
                location = base / "modules" / name
                attempts.append(str(location))
                if location.is_dir():
                    candidate = GitSource(gitdir=location)
                    if candidate.has_commit(sha):
                        return candidate
        raise RuntimeError(f"Missing local gitlink commit {sha} at {path}; attempted {attempts}")

    def blobs(self, entries):
        if not entries:
            return []
        raw = self.run("cat-file", "--batch",
                       data=("".join(entry["blob"] + "\n" for entry in entries)).encode())
        offset = 0
        results = []
        for entry in entries:
            end = raw.index(b"\n", offset)
            oid, kind, size = raw[offset:end].decode().split()
            assert oid == entry["blob"] and kind == "blob"
            start = end + 1
            finish = start + int(size)
            assert raw[finish:finish + 1] == b"\n"
            results.append((entry, raw[start:finish]))
            offset = finish + 1
        assert offset == len(raw)
        return results


def scope_reason(path, is_link=False):
    parts = PurePosixPath(path).parts
    if not parts or (parts[0] not in {"src", "native", "crates"} and path != "build.rs"):
        return "outside_production_roots"
    for part in parts[:-1] if not is_link else parts:
        if part.lower() in EXCLUDED_DIRS:
            return "excluded_directory:" + part.lower()
    if is_link:
        return None
    if PurePosixPath(path).suffix.lower() not in EXTENSIONS:
        return "unsupported_extension:" + (PurePosixPath(path).suffix.lower() or "(none)")
    if TEST_FILE.match(parts[-1]):
        return "dedicated_test_filename"
    if GENERATED_FILE.search(parts[-1]):
        return "generated_filename"
    return None


def component(mount, relative):
    parts = PurePosixPath(relative).parts
    if parts and parts[0] == "crates" and len(parts) > 1:
        crate = PurePosixPath(*parts[:2])
        return str(PurePosixPath(mount) / crate) if mount else str(crate)
    return mount or "ztty-host"


def metrics(files):
    return {"files": len(files), "lines": sum(file["lines"] for file in files),
            "bytes": sum(file["bytes"] for file in files)}


def snapshot(root, sha, top):
    files, excluded, sources, links = [], [], [], []

    def visit(repo, revision, mount="", depth=0, active=()):
        key = (str(repo.gitdir), revision)
        if depth > 32 or key in active:
            raise RuntimeError("Recursive source graph exceeds the bounded traversal policy")
        if not repo.has_commit(revision):
            raise RuntimeError(f"Missing commit {revision} in {repo.gitdir}")
        entries = repo.tree(revision)
        modules = repo.modules(revision)
        source = {"mount": mount or ".", "commit": revision, "depth": depth,
                  "git_dir": str(repo.gitdir),
                  "tree": repo.run("rev-parse", revision + "^{tree}").decode().strip()}
        sources.append(source)
        selected = []
        for entry in entries:
            relative = entry["path"]
            mounted = str(PurePosixPath(mount) / relative) if mount else relative
            owner = "root_repository" if depth == 0 else "gitlink_library"
            record = {**entry, "path": mounted, "relative_path": relative,
                      "component": component(mount, relative), "owner": owner,
                      "source_sha": revision, "source_mount": mount or "."}
            if entry["mode"] == "160000":
                reason = scope_reason(relative, is_link=True)
                if reason:
                    excluded.append({**record, "reason": reason})
                    continue
                module = modules.get(relative, {})
                links.append({"mount": mounted, "commit": entry["blob"],
                              "parent_source_sha": revision, "url": module.get("url")})
                visit(repo.child(relative, entry["blob"], module), entry["blob"], mounted,
                      depth + 1, (*active, key))
                continue
            reason = scope_reason(relative)
            if entry["mode"] == "120000":
                reason = "symlink_not_followed"
            elif entry["type"] != "blob":
                reason = "not_a_source_blob"
            if reason:
                excluded.append({**record, "reason": reason})
            else:
                selected.append(record)
        for record, data in repo.blobs(selected):
            line_count = data.count(b"\n") + int(bool(data) and not data.endswith(b"\n"))
            marker_lines = []
            if PurePosixPath(record["path"]).suffix.lower() == ".rs":
                marker_lines = [number for number, line in enumerate(data.split(b"\n"), 1)
                                if INLINE_MARKER.search(line)]
            files.append({**record, "lines": line_count, "bytes": len(data),
                          "sha256": hashlib.sha256(data).hexdigest(),
                          "inline_test_marker_lines": marker_lines})

    visit(root, sha)
    files.sort(key=lambda value: value["path"])
    excluded.sort(key=lambda value: value["path"])
    assert len({file["path"] for file in files}) == len(files)
    groups = defaultdict(list)
    for file in files:
        groups[file["component"]].append(file)
    omitted = defaultdict(list)
    for entry in excluded:
        omitted[entry["component"]].append(entry)
    components = []
    for name in sorted(groups.keys() | omitted.keys()):
        values, omissions = groups[name], omitted[name]
        all_entries = values + omissions
        components.append({"component": name, **metrics(values),
                           "owners": sorted({value["owner"] for value in all_entries}),
                           "source_shas": sorted({value["source_sha"] for value in all_entries}),
                           "excluded_paths": len(omissions),
                           "exclusion_summary": dict(sorted(Counter(
                               value["reason"] for value in omissions).items())),
                           "files_with_inline_test_markers": sum(
                               bool(value["inline_test_marker_lines"]) for value in values)})
    root_files = [file for file in files if file["owner"] == "root_repository"]
    library_files = [file for file in files if file["owner"] == "gitlink_library"]
    return {"root_sha": sha, "complete": True, "sources": sources, "gitlinks": links,
            "totals": {"root_repository": metrics(root_files),
                       "gitlink_libraries": metrics(library_files), "combined": metrics(files)},
            "components": components,
            "largest_files": sorted(files, key=lambda value: (-value["lines"], value["path"]))[:top],
            "files": files, "excluded": excluded,
            "exclusion_summary": dict(sorted(Counter(item["reason"] for item in excluded).items())),
            "inline_test_files": [file for file in files if file["inline_test_marker_lines"]]}


def render(report):
    before, after = report["before"], report["after"]
    lines = ["# 固定源码规模比较", "",
             f"- Before: {before['root_sha']}", f"- After: {after['root_sha']}",
             f"- 审计脚本 SHA-256: {report['script']['sha256']}", "",
             "统计 Git 对象中的 .rs/.c/.h；物理行包含空白、注释和内联测试。"
             "根仓库直接维护代码与递归 gitlink 库代码分列。此数值不与旧 37848 基数直接相减。", "",
             "| 范围 | 前文件数 | 后文件数 | 前行数 | 后行数 | 行数变化 |",
             "|---|---:|---:|---:|---:|---:|"]
    for key, label in (("root_repository", "根仓库直接维护"),
                       ("gitlink_libraries", "递归 gitlink 库"), ("combined", "合计")):
        a, b = before["totals"][key], after["totals"][key]
        lines.append(f"| {label} | {a['files']} | {b['files']} | {a['lines']} | {b['lines']} | {b['lines']-a['lines']:+d} |")
    lines += ["", "## 组件", "", "| 组件 | 前归属 | 后归属 | 前文件/行 | 后文件/行 | 前/后排除路径 |",
              "|---|---|---|---:|---:|---:|"]
    a = {item["component"]: item for item in before["components"]}
    b = {item["component"]: item for item in after["components"]}
    for name in sorted(a.keys() | b.keys()):
        old, new = a.get(name, {}), b.get(name, {})
        lines.append(f"| {name} | {','.join(old.get('owners', [])) or '—'} | {','.join(new.get('owners', [])) or '—'} | {old.get('files',0)}/{old.get('lines',0)} | {new.get('files',0)}/{new.get('lines',0)} | {old.get('excluded_paths',0)}/{new.get('excluded_paths',0)} |")
    for label, value in (("Before", before), ("After", after)):
        lines += ["", f"## {label} 最大文件", "", "| 文件 | 归属 | 物理行数 | 内联测试文本标记 |",
                  "|---|---|---:|---|"]
        for file in value["largest_files"]:
            lines.append(f"| {file['path']} | {file['owner']} | {file['lines']} | {'有，全部计入' if file['inline_test_marker_lines'] else '未检出标记'} |")
        lines += ["", f"## {label} 排除项汇总", "", "| 原因 | 路径数 |", "|---|---:|"]
        for reason, count in value["exclusion_summary"].items():
            lines.append(f"| {reason} | {count} |")
    lines += ["", "## 核验边界", "",
              "audit.json 包含每个根仓库/子模块的完整提交与 tree SHA、每个计入文件的 Git blob、SHA-256、"
              "物理行数，以及逐路径排除原因。源码来自固定 Git 对象，不读取工作树文件来代替提交。",
              "内联测试标记只作披露，不解析 Rust、不删除测试代码；未检出标记不保证文件没有测试。",
              "仅遍历生产路径中的 gitlink；不展开 crates.io 或系统库依赖。同一库多处挂载时分别计数。",
              "依赖拆出根仓库属于维护归属变化，不能据此声称整个递归源码规模等量减少。", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", required=True)
    parser.add_argument("--after", required=True)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--top", type=int, default=10)
    args = parser.parse_args()
    if not all(re.fullmatch(r"[0-9a-f]{40}", sha) for sha in (args.before, args.after)):
        parser.error("before and after must be full lowercase commit SHAs")
    if not 1 <= args.top <= 100:
        parser.error("--top must be within 1..100")
    if args.output.exists():
        parser.error("--output must be a new directory")
    args.output.mkdir(parents=True)
    script = Path(__file__).resolve()
    report = {"schema": 1, "complete": False, "rules": RULES,
              "recorded_utc": datetime.now(timezone.utc).isoformat(),
              "script": {"path": str(script), "sha256": hashlib.sha256(script.read_bytes()).hexdigest()},
              "requested": {"before": args.before, "after": args.after, "repo": str(args.repo.resolve())}}
    try:
        root = GitSource(worktree=args.repo)
        report["before"] = snapshot(root, args.before, args.top)
        report["after"] = snapshot(root, args.after, args.top)
        old = {item["path"]: item for item in report["before"]["files"]}
        new = {item["path"]: item for item in report["after"]["files"]}
        report["same_blob_owner_changes"] = [
            {"path": path, "blob": old[path]["blob"], "lines": old[path]["lines"],
             "before_owner": old[path]["owner"], "after_owner": new[path]["owner"]}
            for path in sorted(old.keys() & new.keys())
            if old[path]["blob"] == new[path]["blob"] and old[path]["owner"] != new[path]["owner"]]
        report["complete"] = True
        (args.output / "SUMMARY.md").write_text(render(report))
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        (args.output / "audit.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    summary = {"complete": report["complete"], "output": str(args.output),
               "before": report.get("before", {}).get("totals"),
               "after": report.get("after", {}).get("totals")}
    if report.get("error"):
        summary["error"] = report["error"]
    print(json.dumps(summary, ensure_ascii=False))
    return 0 if report["complete"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
