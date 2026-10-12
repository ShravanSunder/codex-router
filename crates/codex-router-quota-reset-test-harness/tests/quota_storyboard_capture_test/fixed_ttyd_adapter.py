#!/usr/bin/python3
"""Restrict VHS's PTY child to one preconfigured guarded quota executable."""
import json
import os
import pathlib
import sys

configuration = json.loads(pathlib.Path(os.environ["QUOTA_STORYBOARD_FIXTURE_COMMAND"]).read_text())
arguments = sys.argv[1:]
environment = dict(os.environ)
environment["DYLD_LIBRARY_PATH"] = configuration["dynamic_library_path"]
if arguments == ["--version"]:
    os.execve(configuration["ttyd"], [configuration["ttyd"], "--version"], environment)
output = pathlib.Path(configuration["output"])
(output / "ttyd-invocation.json").write_text(json.dumps(arguments))
diagnostic_descriptor = os.open(output / "ttyd.stderr", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
os.dup2(diagnostic_descriptor, 2)
os.close(diagnostic_descriptor)
expected_tail = [
    "--interface", "127.0.0.1", "-t", "rendererType=canvas",
    "-t", "disableResizeOverlay=true", "-t", "enableSixel=true",
    "-t", "customGlyphs=true", "--once", "--writable",
    "bash", "--noprofile", "--norc", "--login", "+o", "history",
]
if not arguments or not arguments[0].startswith("--port=") or arguments[1:] != expected_tail:
    raise SystemExit("Rejected unexpected VHS ttyd invocation")
port = int(arguments[0].removeprefix("--port="))
if not 0 < port < 65536:
    raise SystemExit("Rejected invalid capture port")
(output / "ttyd.pid").write_text(str(os.getpid()))
os.execve(configuration["ttyd"], [configuration["ttyd"], *arguments[:13],
    "/usr/bin/python3", str(output / "fixed_quota_child.py")], environment)
