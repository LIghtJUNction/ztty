#!/usr/bin/env python3
"""Package a supplied native Mach-O executable as ztty.app and release archives.

Example (no compiler or network access is used):
    python3 tools/package_macos.py --binary target/release/ztty --output dist \
        --version 0.2.0 --arch aarch64

On macOS, --codesign auto applies and verifies an ad-hoc signature. This is not
Developer ID signing or notarization. Elsewhere auto preserves the supplied
binary without claiming that its existing signature is valid.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

IDENTIFIER = "com.lightjunction.ztty"
CPU_ARCH = {0x01000007: "x86_64", 0x0100000C: "aarch64"}
THIN_MAGIC = {b"\xcf\xfa\xed\xfe": "<", b"\xfe\xed\xfa\xcf": ">"}
FAT_MAGIC = {
    b"\xca\xfe\xba\xbe": (">", False), b"\xbe\xba\xfe\xca": ("<", False),
    b"\xca\xfe\xba\xbf": (">", True), b"\xbf\xba\xfe\xca": ("<", True),
}


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def version_tuple(value: str) -> tuple[int, int, int]:
    if not re.fullmatch(r"\d+\.\d+(?:\.\d+)?", value):
        raise ValueError(f"invalid macOS version: {value!r}")
    parts = tuple(map(int, value.split(".")))
    return (*parts, 0) if len(parts) == 2 else parts


def packed_version(value: int) -> str:
    return f"{value >> 16}.{(value >> 8) & 255}.{value & 255}"


def inspect_slice(stream, offset: int, size: int) -> dict:
    stream.seek(offset)
    header = stream.read(32)
    if len(header) != 32 or header[:4] not in THIN_MAGIC:
        raise ValueError("expected a 64-bit Mach-O slice")
    endian = THIN_MAGIC[header[:4]]
    _, cpu, subtype, file_type, commands, command_bytes, _, _ = struct.unpack(endian + "8I", header)
    if cpu not in CPU_ARCH:
        raise ValueError(f"unsupported Mach-O CPU type 0x{cpu:x}")
    if file_type != 2:
        raise ValueError("Mach-O input must be MH_EXECUTE, not a library or object")
    if commands == 0 or commands > 8192 or command_bytes > 16 * 1024 * 1024 or 32 + command_bytes > size:
        raise ValueError("invalid Mach-O load-command bounds")
    data = stream.read(command_bytes)
    if len(data) != command_bytes:
        raise ValueError("truncated Mach-O load commands")
    cursor = 0
    libraries: list[str] = []
    minimum = "0.0.0"
    signed = False
    has_entry = False
    for _ in range(commands):
        if cursor + 8 > len(data):
            raise ValueError("truncated Mach-O load command")
        command, length = struct.unpack_from(endian + "II", data, cursor)
        if length < 8 or length % 8 or cursor + length > len(data):
            raise ValueError("invalid Mach-O load-command size")
        body = data[cursor:cursor + length]
        if command in (0x0C, 0x80000018, 0x8000001F, 0x80000023):
            if length < 24:
                raise ValueError("truncated dylib load command")
            name_offset, = struct.unpack_from(endian + "I", body, 8)
            if name_offset < 24 or name_offset >= length or b"\0" not in body[name_offset:]:
                raise ValueError("invalid dylib path")
            name = body[name_offset:].split(b"\0", 1)[0].decode("utf-8", errors="strict")
            if not name.startswith(("/usr/lib/", "/System/Library/Frameworks/", "/System/Library/PrivateFrameworks/")):
                raise ValueError(f"unbundled non-system dependency: {name}")
            libraries.append(name)
        elif command == 0x32:  # LC_BUILD_VERSION
            if length < 24:
                raise ValueError("truncated LC_BUILD_VERSION")
            target, deployment, _, tools = struct.unpack_from(endian + "4I", body, 8)
            if target != 1:
                raise ValueError("Mach-O input is not built for macOS")
            if 24 + tools * 8 > length:
                raise ValueError("invalid LC_BUILD_VERSION tool count")
            minimum = max(minimum, packed_version(deployment), key=version_tuple)
        elif command == 0x24:  # LC_VERSION_MIN_MACOSX
            if length < 16:
                raise ValueError("truncated LC_VERSION_MIN_MACOSX")
            deployment, = struct.unpack_from(endian + "I", body, 8)
            minimum = max(minimum, packed_version(deployment), key=version_tuple)
        elif command == 0x80000028:  # LC_MAIN
            if length != 24:
                raise ValueError("invalid LC_MAIN size")
            entry_offset, _ = struct.unpack_from(endian + "QQ", body, 8)
            if entry_offset >= size:
                raise ValueError("Mach-O entry point lies outside its slice")
            has_entry = True
        elif command == 0x05:  # LC_UNIXTHREAD (older macOS executables)
            if length < 16:
                raise ValueError("truncated LC_UNIXTHREAD")
            has_entry = True
        elif command == 0x1D:  # LC_CODE_SIGNATURE
            if length < 16:
                raise ValueError("truncated LC_CODE_SIGNATURE")
            signature_offset, signature_size = struct.unpack_from(endian + "II", body, 8)
            if signature_offset + signature_size > size:
                raise ValueError("code signature lies outside the Mach-O slice")
            signed = signature_size != 0
        cursor += length
    if cursor != len(data) or minimum == "0.0.0" or not has_entry:
        raise ValueError("inconsistent load commands or missing macOS deployment version/entry point")
    return {"architecture": CPU_ARCH[cpu], "cpu_type": cpu, "cpu_subtype": subtype,
            "minimum_macos": minimum, "libraries": sorted(set(libraries)),
            "has_code_signature_command": signed}


def inspect_macho(path: Path) -> list[dict]:
    size = path.stat().st_size
    if not path.is_file() or not 32 <= size <= 256 * 1024 * 1024:
        raise ValueError("input must be a regular Mach-O file of at most 256 MiB")
    with path.open("rb") as stream:
        magic = stream.read(4)
        if magic in THIN_MAGIC:
            return [inspect_slice(stream, 0, size)]
        if magic not in FAT_MAGIC:
            raise ValueError("input is not a supported Mach-O executable")
        endian, fat64 = FAT_MAGIC[magic]
        count_data = stream.read(4)
        if len(count_data) != 4:
            raise ValueError("truncated universal Mach-O header")
        count, = struct.unpack(endian + "I", count_data)
        if count < 1 or count > 2:
            raise ValueError("only x86_64/arm64 universal executables are supported")
        entry_size = 32 if fat64 else 20
        table_end = 8 + count * entry_size
        slices: list[tuple[int, int, int]] = []
        for _ in range(count):
            row = stream.read(entry_size)
            if len(row) != entry_size:
                raise ValueError("truncated universal architecture table")
            fields = struct.unpack(endian + ("IIQQII" if fat64 else "IIIII"), row)
            cpu, _, offset, length, alignment = fields[:5]
            if cpu not in CPU_ARCH or offset < table_end or length < 32 or offset + length > size or alignment > 31 or offset % (1 << alignment):
                raise ValueError("invalid universal Mach-O slice bounds")
            if any(offset < previous + previous_size and previous < offset + length for _, previous, previous_size in slices):
                raise ValueError("overlapping universal Mach-O slices")
            slices.append((cpu, offset, length))
        result = []
        for cpu, offset, length in slices:
            details = inspect_slice(stream, offset, length)
            if details["cpu_type"] != cpu:
                raise ValueError("universal architecture table and slice disagree")
            result.append(details)
        if len({item["architecture"] for item in result}) != len(result):
            raise ValueError("duplicate universal architecture")
        return sorted(result, key=lambda item: item["architecture"])


def file_manifest(bundle: Path) -> dict[str, dict]:
    return {str(path.relative_to(bundle.parent)): {
        "bytes": path.stat().st_size, "sha256": sha256(path),
        "mode": f"{stat.S_IMODE(path.stat().st_mode):04o}",
    } for path in sorted(bundle.rglob("*")) if path.is_file()}


def verify_bundle(bundle: Path, version: str, architectures: list[str]) -> dict[str, dict]:
    with (bundle / "Contents/Info.plist").open("rb") as stream:
        info = plistlib.load(stream)
    required = {"CFBundleIdentifier": IDENTIFIER, "CFBundleExecutable": "ztty",
                "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version,
                "CFBundleVersion": version, "NSHighResolutionCapable": True}
    if any(info.get(key) != value for key, value in required.items()):
        raise ValueError("generated Info.plist failed its metadata check")
    executable = bundle / "Contents/MacOS/ztty"
    if not executable.is_file() or executable.is_symlink() or not stat.S_IMODE(executable.stat().st_mode) & 0o111:
        raise ValueError("app bundle has no regular executable")
    if sorted(item["architecture"] for item in inspect_macho(executable)) != sorted(architectures):
        raise ValueError("packaged Mach-O architectures changed")
    if not (bundle / "Contents/Resources/LICENSE").read_text(encoding="utf-8").startswith("MIT License"):
        raise ValueError("app bundle is missing its MIT license")
    if any(path.is_symlink() for path in bundle.rglob("*")):
        raise ValueError("unexpected symlink in generated app bundle")
    return file_manifest(bundle)


def archive_zip(bundle: Path, destination: Path, epoch: int, files: dict[str, dict]) -> None:
    stamp = datetime.fromtimestamp(max(315532800, min(epoch, 4354819199)), timezone.utc).timetuple()[:6]
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, details in files.items():
            path = bundle.parent / name
            entry = zipfile.ZipInfo(name, date_time=stamp)
            entry.create_system = 3
            entry.external_attr = (stat.S_IFREG | int(details["mode"], 8)) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(entry, path.read_bytes())
    with zipfile.ZipFile(destination) as archive:
        if set(archive.namelist()) != set(files) or archive.testzip() is not None:
            raise ValueError("ZIP archive member/CRC verification failed")
        for name, details in files.items():
            if hashlib.sha256(archive.read(name)).hexdigest() != details["sha256"] or stat.S_IMODE(archive.getinfo(name).external_attr >> 16) != int(details["mode"], 8):
                raise ValueError(f"ZIP round trip changed {name}")


def archive_tar(bundle: Path, destination: Path, epoch: int, files: dict[str, dict]) -> None:
    with destination.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=epoch) as compressed, tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for path in [bundle, *sorted(bundle.rglob("*"))]:
            name = str(path.relative_to(bundle.parent))
            info = archive.gettarinfo(str(path), arcname=name)
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = epoch
            if path.is_file():
                with path.open("rb") as stream:
                    archive.addfile(info, stream)
            else:
                archive.addfile(info)
    with tarfile.open(destination, "r:gz") as archive:
        members = {member.name: member for member in archive.getmembers() if member.isfile()}
        if set(members) != set(files):
            raise ValueError("tar archive member verification failed")
        for name, details in files.items():
            stream = archive.extractfile(members[name])
            if stream is None or hashlib.file_digest(stream, "sha256").hexdigest() != details["sha256"] or members[name].mode != int(details["mode"], 8):
                raise ValueError(f"tar round trip changed {name}")


def run_checked(command: list[str]) -> None:
    result = subprocess.run(command, capture_output=True, text=True, timeout=60)
    if result.returncode:
        raise ValueError(f"{command[0]} failed ({result.returncode}): {(result.stderr or result.stdout)[-4000:]}")


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="output directory for ztty.app and archives")
    parser.add_argument("--version", help="defaults to the root Cargo.toml package version")
    parser.add_argument("--arch", choices=["aarch64", "x86_64", "universal2"], help="verify the expected architecture")
    parser.add_argument("--format", choices=["zip", "tar.gz", "both"], default="both")
    parser.add_argument("--minimum-system-version", default="11.0")
    parser.add_argument("--codesign", choices=["auto", "ad-hoc", "none"], default="auto")
    parser.add_argument("--force", action="store_true", help="replace only existing ztty output artifacts")
    args = parser.parse_args()
    version = args.version or tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    match = re.fullmatch(r"(\d+\.\d+\.\d+)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version)
    if not match:
        parser.error("version must be a semantic version with three numeric components")
    numeric_version = match.group(1)
    binary = args.binary.resolve(strict=True)
    slices = inspect_macho(binary)
    architectures = [item["architecture"] for item in slices]
    architecture = architectures[0] if len(architectures) == 1 else "universal2"
    if args.arch and args.arch != architecture:
        parser.error(f"expected {args.arch}, supplied Mach-O contains {architecture}")
    minimum = max([args.minimum_system_version, *(item["minimum_macos"] for item in slices)], key=version_tuple)
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", int(binary.stat().st_mtime)))
    if not 0 <= epoch <= 0xFFFFFFFF:
        parser.error("SOURCE_DATE_EPOCH must fit an unsigned 32-bit timestamp")
    sign = args.codesign == "ad-hoc" or args.codesign == "auto" and sys.platform == "darwin"
    if sign and (sys.platform != "darwin" or shutil.which("codesign") is None):
        parser.error("ad-hoc signing requires native macOS codesign")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    stem = f"ztty-macos-{architecture}"
    formats = ["zip", "tar.gz"] if args.format == "both" else [args.format]
    names = ["ztty.app", f"{stem}.manifest.json", f"{stem}.manifest.json.sha256"]
    names += [f"{stem}.{suffix}{checksum}" for suffix in formats for checksum in ("", ".sha256")]
    for name in names:
        target = output / name
        if target.exists() or target.is_symlink():
            if not args.force:
                parser.error(f"output already exists: {target}; use --force to replace this package")
            if target.is_symlink():
                parser.error(f"will not replace symlink: {target}")
            if target.is_dir():
                info_path = target / "Contents/Info.plist"
                if name != "ztty.app" or not info_path.is_file() or plistlib.loads(info_path.read_bytes()).get("CFBundleIdentifier") != IDENTIFIER:
                    parser.error(f"will not replace unrelated directory: {target}")
    with tempfile.TemporaryDirectory(prefix=".ztty-macos-", dir=output) as temporary:
        staging = Path(temporary)
        bundle = staging / "ztty.app"
        executable = bundle / "Contents/MacOS/ztty"
        resources = bundle / "Contents/Resources"
        executable.parent.mkdir(parents=True)
        resources.mkdir()
        shutil.copyfile(binary, executable)
        executable.chmod(0o755)
        shutil.copyfile(root / "LICENSE", resources / "LICENSE")
        copyright_line = next((line for line in (root / "LICENSE").read_text(encoding="utf-8").splitlines() if line.startswith("Copyright")), "Copyright LIghtJUNction")
        info = {
            "CFBundleInfoDictionaryVersion": "6.0", "CFBundleIdentifier": IDENTIFIER,
            "CFBundleName": "ztty", "CFBundleDisplayName": "ztty", "CFBundleExecutable": "ztty",
            "CFBundlePackageType": "APPL", "CFBundleShortVersionString": numeric_version,
            "CFBundleVersion": numeric_version, "CFBundleDevelopmentRegion": "en",
            "CFBundleSupportedPlatforms": ["MacOSX"], "NSPrincipalClass": "NSApplication",
            "NSHighResolutionCapable": True, "NSSupportsAutomaticGraphicsSwitching": True,
            "NSHumanReadableCopyright": copyright_line + ". MIT License.",
            "LSMinimumSystemVersion": minimum, "LSApplicationCategoryType": "public.app-category.developer-tools",
            "LSArchitecturePriority": ["arm64" if arch == "aarch64" else arch for arch in architectures],
        }
        (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps(info, fmt=plistlib.FMT_XML, sort_keys=True))
        (resources / "BUILD.json").write_text(json.dumps({"version": version, "architectures": architectures,
            "source_binary_sha256": sha256(binary), "source_binary_bytes": binary.stat().st_size,
            "minimum_macos": minimum, "license": "MIT"}, indent=2) + "\n", encoding="utf-8")
        for path in bundle.rglob("*"):
            if path.is_dir():
                path.chmod(0o755)
            elif path != executable:
                path.chmod(0o644)
        bundle.chmod(0o755)
        if sign:
            run_checked(["codesign", "--force", "--sign", "-", "--timestamp=none", str(bundle)])
            run_checked(["codesign", "--verify", "--strict", "--verbose=2", str(bundle)])
        files = verify_bundle(bundle, numeric_version, architectures)
        archives = {}
        for suffix in formats:
            archive_path = staging / f"{stem}.{suffix}"
            (archive_zip if suffix == "zip" else archive_tar)(bundle, archive_path, epoch, files)
            archives[archive_path.name] = {"sha256": sha256(archive_path), "bytes": archive_path.stat().st_size}
            (staging / (archive_path.name + ".sha256")).write_text(f"{sha256(archive_path)}  {archive_path.name}\n", encoding="ascii")
        manifest = {"version": version, "bundle_identifier": IDENTIFIER, "architecture": architecture,
            "minimum_macos": minimum, "source_slices": slices, "packaged_slices": inspect_macho(executable),
            "signature": "ad-hoc-verified" if sign else "not-re-signed-or-verified",
            "notarized": False, "files": files, "archives": archives,
            "validation": "Mach-O structure, bundle metadata, file hashes and archive round trips checked; runtime and displayed pixels are separate tests."}
        manifest_path = staging / f"{stem}.manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        (staging / (manifest_path.name + ".sha256")).write_text(f"{sha256(manifest_path)}  {manifest_path.name}\n", encoding="ascii")
        for name in names:
            target = output / name
            if target.is_dir():
                shutil.rmtree(target)
            os.replace(staging / name, target)
    print(json.dumps({"output": str(output), "bundle": str(output / "ztty.app"), "manifest": str(output / f"{stem}.manifest.json"), "architecture": architecture, "signature": manifest["signature"], "archives": archives}, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"package_macos: {error}", file=sys.stderr)
        raise SystemExit(1)
