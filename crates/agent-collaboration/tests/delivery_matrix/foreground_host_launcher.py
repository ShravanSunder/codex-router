#!/usr/bin/env python3
"""Publish this process identity, then exec the real isolated foreground CLI."""

import json
import os
from pathlib import Path
import stat
import sys


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(message)


require(len(sys.argv) == 2, "expected one absolute codex-router executable path")
root_argument = os.environ.get("CODEX_AUTOMATION_PROOF_ROOT")
require(root_argument is not None, "CODEX_AUTOMATION_PROOF_ROOT is required")
root_path = Path(root_argument)
require(root_path.is_absolute() and root_path.parent == Path("/tmp"), "fixture root must be a direct /tmp child")
root = root_path.resolve(strict=True)
require(root.parent == Path("/tmp").resolve(strict=True), "fixture root escaped /tmp")
require(stat.S_IMODE(root.stat().st_mode) & 0o077 == 0, "fixture root must remain owner-private")

cli_argument = Path(sys.argv[1])
require(cli_argument.is_absolute(), "codex-router path must be absolute")
cli = cli_argument.resolve(strict=True)
require(cli.is_file() and os.access(cli, os.X_OK), "codex-router must be an executable regular file")

marker_path = root / "debug-host-context.json"
marker_metadata = marker_path.lstat()
require(stat.S_ISREG(marker_metadata.st_mode), "fixture marker must be a regular file")
require(marker_metadata.st_uid == os.geteuid(), "fixture marker must belong to the current effective user")
require(stat.S_IMODE(marker_metadata.st_mode) & 0o077 == 0, "fixture marker must remain owner-private")
with marker_path.open("r", encoding="utf-8") as marker_file:
    marker = json.load(marker_file)
require(marker.get("kind") == "isolatedDeliveryMatrix", "fixture marker kind mismatch")
require(marker.get("cliExecutable") == str(cli), "fixture marker executable mismatch")
require(marker.get("port") == 43127, "fixture Router port mismatch")

socket_directory = root / "native-socket"
socket_directory_metadata = socket_directory.lstat()
require(stat.S_ISDIR(socket_directory_metadata.st_mode), "native socket directory must be real")
require(socket_directory_metadata.st_uid == os.geteuid(), "native socket directory owner mismatch")
require(stat.S_IMODE(socket_directory_metadata.st_mode) & 0o077 == 0, "native socket directory must remain private")

# Publish the current Python PID immediately before replacing this process image.
marker.update({"runDirectory": str(root), "hostPid": os.getpid()})
temporary_path = root / ".debug-host-context-launch.tmp"
descriptor = os.open(temporary_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
try:
    with os.fdopen(descriptor, "w", encoding="utf-8") as temporary_file:
        json.dump(marker, temporary_file, separators=(",", ":"))
        temporary_file.flush()
        os.fsync(temporary_file.fileno())
    os.replace(temporary_path, marker_path)
except BaseException:
    try:
        os.unlink(temporary_path)
    except FileNotFoundError:
        pass
    raise

environment = {
    "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "HOME": str(root / "home"),
    "CODEX_HOME": str(root / "codex-home"),
    "CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET": str(socket_directory / "app-server.sock"),
}
user = os.environ.get("USER")
if user:
    environment["USER"] = user

arguments = [
    str(cli),
    "host",
    "--router-root",
    str(root),
    "--port",
    "43127",
    "--mcp-bind",
    "127.0.0.1:43128",
    "--require-debug-isolation",
]
os.execve(cli, arguments, environment)
